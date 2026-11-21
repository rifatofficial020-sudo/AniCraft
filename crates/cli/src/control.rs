//! The live control channel (ADR 0104, `docs/control-protocol.md`): the desktop app's control
//! port, and the client `anicraft-cli mcp --bridge <port>` uses to reach it.
//!
//! The app listens on `127.0.0.1` only. Each connection carries one JSON object per line each
//! way, answered in order. Every request carries the run's token, which the app writes to
//! `control.json` in the AniCraft settings folder, so a client running as the same user finds it
//! with no setup. The tools are the MCP server's ([`crate::mcp::tools`], [`crate::mcp::call_tool`]),
//! so live and headless never drift.
//!
//! Nothing here touches the window: [`listen`] hands each tool call to whoever drains
//! [`Server::try_recv`] (the app's frame loop), with a channel for the answer.

use crate::mcp;
use anicraft_engine::access::Roots;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

/// The wire protocol's version, reported by `hello`. 2: `tools.call` carries the caller's
/// automation roots, and a call without them may touch no file (ADR 0124).
pub const PROTOCOL: u32 = 2;
/// The port AniCraft's documentation and shortcuts use (VectorCraft 7979, CadCraft 7980,
/// FilmCraft 9876, EffectCraft 9877).
pub const DEFAULT_PORT: u16 = 9878;
/// Longest request or reply line, in bytes.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Most connections open at once.
pub const MAX_CONNECTIONS: usize = 16;
/// How long a tool call may wait for the app (a frame, or the end of the person's drag).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Extra time the connection waits past the deadline: a call that started just before it
/// still gets its real answer.
const GRACE: Duration = Duration::from_secs(10);
/// How long a new connection may take to send its first request with the right token. Past it
/// the connection closes, so a program that connects and says nothing can't hold one of the
/// [`MAX_CONNECTIONS`] places.
pub const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Overrides the generated token on both sides (CI, power users).
pub const TOKEN_ENV: &str = "ANICRAFT_CONTROL_TOKEN";
/// Where the bridge reads `control.json` from, when it isn't in the settings folder.
pub const FILE_ENV: &str = "ANICRAFT_CONTROL_FILE";
/// The port when `--control` isn't given.
pub const PORT_ENV: &str = "ANICRAFT_CONTROL_PORT";
/// The file the app writes in the settings folder while its control port is open.
pub const CONTROL_FILE: &str = "control.json";

/// AniCraft's settings folder: %APPDATA%\AniCraft, ~/Library/Application Support/AniCraft, or
/// $XDG_CONFIG_HOME/anicraft (default ~/.config/anicraft). The shell's `platform::config_dir`
/// owns this rule; a shell test keeps the two the same (this crate can't use the shell).
pub fn settings_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        env("APPDATA").map(|p| p.join("AniCraft"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|p| p.join("Library").join("Application Support").join("AniCraft"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config"))).map(|p| p.join("anicraft"))
    }
}

/// Where the bridge looks for `control.json`: `ANICRAFT_CONTROL_FILE` when set (a portable
/// install keeps the file in its own data folder, which this crate can't find), else the
/// settings folder.
pub fn control_file() -> Option<PathBuf> {
    std::env::var_os(FILE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| settings_dir().map(|d| d.join(CONTROL_FILE)))
}

/// `ANICRAFT_CONTROL_TOKEN`, when set and not blank.
pub fn token_from_env() -> Option<String> {
    std::env::var(TOKEN_ENV).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// A fresh token: 32 random bytes from the operating system, as 64 hex digits.
pub fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers from the system: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Reads a port: a number from 1 to 65535.
pub fn parse_port(s: &str) -> Result<u16, String> {
    match s.trim().parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(format!("'{s}' is not a port: use a number from 1 to 65535, such as {DEFAULT_PORT}")),
    }
}

/// What `control.json` holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlFile {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub version: String,
}

impl ControlFile {
    pub fn to_json(&self) -> Value {
        json!({"port": self.port, "token": self.token, "pid": self.pid, "version": self.version})
    }
    pub fn parse(text: &str) -> Result<ControlFile, String> {
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{CONTROL_FILE} is not JSON: {e}"))?;
        let token = v.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| format!("{CONTROL_FILE} has no token"))?;
        let port = v.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).ok_or_else(|| format!("{CONTROL_FILE} has no port"))?;
        let pid = v.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()).unwrap_or(0);
        let version = v.get("version").and_then(Value::as_str).unwrap_or("").to_string();
        Ok(ControlFile { port, token: token.to_string(), pid, version })
    }
}

/// Why a line could not be read.
#[derive(Debug)]
pub enum LineError {
    TooLong,
    NotText,
    Io(std::io::Error),
}

/// Reads one line (without its line break), at most `limit` bytes; `Ok(None)` at the end.
pub fn read_line<R: BufRead>(r: &mut R, limit: usize) -> Result<Option<String>, LineError> {
    let mut buf = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let n = r.by_ref().take(cap).read_until(b'\n', &mut buf).map_err(LineError::Io)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > limit {
        return Err(LineError::TooLong);
    }
    if buf.len() > limit {
        return Err(LineError::TooLong);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|_| LineError::NotText)
}

/// One request off the wire.
#[derive(Debug, PartialEq)]
pub struct Incoming {
    pub id: Value,
    pub token: Option<String>,
    pub method: String,
    pub params: Value,
}

/// Reads a request line: a JSON object with a string `method`. Anything else (an HTTP request
/// line, which is how a web page's `fetch` to localhost begins, included) is refused.
pub fn parse_request(line: &str) -> Result<Incoming, String> {
    let v: Value = serde_json::from_str(line.trim_start_matches('\u{feff}')).map_err(|_| "expected one JSON object per line, such as {\"id\":1,\"token\":\"…\",\"method\":\"hello\"}".to_string())?;
    let Value::Object(mut o) = v else { return Err("expected a JSON object".into()) };
    let method = match o.remove("method") {
        Some(Value::String(m)) => m,
        _ => return Err("the request needs a string \"method\"".into()),
    };
    let token = match o.remove("token") {
        Some(Value::String(t)) => Some(t),
        _ => None,
    };
    Ok(Incoming { id: o.remove("id").unwrap_or(Value::Null), token, method, params: o.remove("params").unwrap_or_else(|| json!({})) })
}

/// Compares tokens in time that doesn't depend on where they differ.
pub fn token_matches(given: Option<&str>, expected: &str) -> bool {
    let Some(given) = given else { return false };
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    a.len() == b.len() && !b.is_empty() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"id": id, "ok": true, "result": result})
}

fn fail(id: &Value, error: impl Into<String>) -> Value {
    json!({"id": id, "ok": false, "error": error.into()})
}

/// The message when a tool call never got its turn.
pub const BUSY: &str = "AniCraft was busy for 60 s (an edit in progress in the window, such as a drag, typing or a plug-in dialog), so this call was not run. Try again.";

/// A tool call waiting for the app's frame loop.
pub struct Request {
    pub name: String,
    pub arguments: Value,
    /// The folders the caller's commands may use; none at all when it sent none.
    roots: Arc<Roots>,
    deadline: Instant,
    reply: Sender<Result<Value, String>>,
}

impl Request {
    /// Whether it can change the document (and so waits for the person's own gesture to end).
    pub fn changes_document(&self) -> bool {
        mcp::tool_changes_document(&self.name)
    }
    /// Past its deadline: it must not run any more (the client was told it didn't).
    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
    /// Runs it on `host` and sends the answer; an expired request is refused instead.
    pub fn run<H: mcp::Host + ?Sized>(self, host: &mut H) {
        let result = if self.expired() { Err(BUSY.to_string()) } else { mcp::call_tool_in(host, &self.name, &self.arguments, Some(&self.roots)) };
        let _ = self.reply.send(result);
    }
    /// Answers without running (the app is closing).
    pub fn refuse(self, why: &str) {
        let _ = self.reply.send(Err(why.to_string()));
    }
}

/// Wakes the app's frame loop (`egui::Context::request_repaint`), from any thread.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// An open control port. Dropping it stops listening and frees the port (a window that opens
/// again in the same run, ADR 0092, can then listen on it again).
pub struct Server {
    pub port: u16,
    rx: Receiver<Request>,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // `accept` only returns for a connection: make one, so the listener thread sees the flag.
        let _ = TcpStream::connect_timeout(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port)), Duration::from_secs(1));
    }
}

impl Server {
    /// The next waiting tool call, if any.
    pub fn try_recv(&self) -> Option<Request> {
        self.rx.try_recv().ok()
    }
}

/// Opens the control port on `127.0.0.1:port`. Tool calls arrive through [`Server::try_recv`];
/// `wake` is called whenever one is waiting.
pub fn listen(port: u16, token: String, wake: Wake) -> std::io::Result<Server> {
    listen_with(port, token, wake, FIRST_REQUEST_TIMEOUT)
}

/// [`listen`], with the time a new connection has to send its first good request.
pub fn listen_with(port: u16, token: String, wake: Wake, first_request: Duration) -> std::io::Result<Server> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = channel();
    let token: Arc<str> = token.into();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::Builder::new().name("anicraft-control".into()).spawn(move || accept_loop(&listener, &token, &tx, &wake, &stopped, first_request))?;
    Ok(Server { port, rx, stop })
}

fn accept_loop(listener: &TcpListener, token: &Arc<str>, tx: &Sender<Request>, wake: &Wake, stop: &AtomicBool, first_request: Duration) {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        // The server was dropped: stop, which closes the listener and frees the port.
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut stream) = stream else { continue };
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(stream, "{}", fail(&Value::Null, format!("too many connections (at most {MAX_CONNECTIONS})")));
            continue;
        }
        let (token, tx, wake, active2) = (token.clone(), tx.clone(), wake.clone(), active.clone());
        let spawned = std::thread::Builder::new().name("anicraft-control-conn".into()).spawn(move || {
            serve_connection(stream, &token, &tx, &wake, first_request);
            active2.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// One connection: requests in order, until the client leaves or sends something it shouldn't.
fn serve_connection(stream: TcpStream, token: &str, tx: &Sender<Request>, wake: &Wake, first_request: Duration) {
    let Ok(read_half) = stream.try_clone() else { return };
    // Until the first request with the right token, reads time out (see the constant).
    let _ = read_half.set_read_timeout(Some(first_request));
    let mut known = false;
    let mut reader = BufReader::new(read_half);
    let mut out = stream;
    loop {
        let line = match read_line(&mut reader, MAX_LINE) {
            Ok(Some(l)) => l,
            Ok(None) | Err(LineError::Io(_)) => return,
            Err(LineError::TooLong) => {
                let _ = writeln!(out, "{}", fail(&Value::Null, format!("request longer than {} MiB", MAX_LINE / (1024 * 1024))));
                return close_after_reply(&out, &mut reader);
            }
            Err(LineError::NotText) => {
                let _ = writeln!(out, "{}", fail(&Value::Null, "the request is not UTF-8 text"));
                return close_after_reply(&out, &mut reader);
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let (reply, keep_open) = answer(&line, token, tx, wake);
        if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
            return;
        }
        if !keep_open {
            return close_after_reply(&out, &mut reader);
        }
        // Only a request with the right token keeps the connection open: from here on the
        // client may be idle for as long as it likes (an assistant between calls).
        if !known {
            known = true;
            // On the reading handle itself: Windows keeps the timeout per handle, not per socket.
            let _ = reader.get_ref().set_read_timeout(None);
        }
    }
}

/// Most a refused client may still send after its reply before the connection is cut, and for
/// how long (see [`close_after_reply`]).
const DRAIN_BYTES: u64 = 2 * MAX_LINE as u64;
const DRAIN_TIME: Duration = Duration::from_secs(2);

/// Closes a connection after a refusal so the client still reads the reply. Closing a socket
/// with unread input makes the system reset the connection, and a reset may discard the reply
/// before the client reads it (macOS does). So: no more writes, then read and drop what the
/// client is still sending, bounded in bytes and time, then close.
fn close_after_reply(out: &TcpStream, reader: &mut BufReader<TcpStream>) {
    let _ = out.shutdown(std::net::Shutdown::Write);
    let _ = reader.get_ref().set_read_timeout(Some(DRAIN_TIME));
    let started = Instant::now();
    let mut buf = [0u8; 64 * 1024];
    let mut left = DRAIN_BYTES;
    // Bytes already buffered count too.
    let buffered = reader.buffer().len();
    reader.consume(buffered);
    while left > 0 && started.elapsed() < DRAIN_TIME {
        match reader.get_mut().read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n as u64),
        }
    }
}

/// The automation roots a `tools.call` carries: `{"read": [folder, …], "write": folder}`,
/// absolute. Without them the call may touch no file at all.
pub fn parse_roots(v: Option<&Value>) -> Result<Roots, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(Roots::none()) };
    let bad = || "\"roots\" must be {\"read\": [folder, …], \"write\": folder} with absolute paths".to_string();
    if !v.is_object() {
        return Err(bad());
    }
    let absolute = |v: &Value| v.as_str().map(PathBuf::from).filter(|p| p.is_absolute()).ok_or_else(bad);
    let read = match v.get("read") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(absolute).collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(bad()),
    };
    let write = match v.get("write") {
        None | Some(Value::Null) => None,
        Some(w) => Some(absolute(w)?),
    };
    Roots::new(&read, write.as_deref())
}

/// The reply to one request line, and whether the connection stays open.
fn answer(line: &str, token: &str, tx: &Sender<Request>, wake: &Wake) -> (Value, bool) {
    let req = match parse_request(line) {
        Ok(r) => r,
        Err(e) => return (fail(&Value::Null, e), false),
    };
    if !token_matches(req.token.as_deref(), token) {
        let why = if req.token.is_none() { "missing token" } else { "wrong token" };
        return (fail(&req.id, format!("{why}: send the \"token\" from {CONTROL_FILE} in AniCraft's settings folder (or {TOKEN_ENV})")), false);
    }
    let id = &req.id;
    match req.method.as_str() {
        "hello" => (ok(id, json!({"app": "AniCraft", "version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL})), true),
        "tools.list" => (ok(id, mcp::tools()), true),
        "tools.call" => {
            let Some(name) = req.params.get("name").and_then(Value::as_str) else { return (fail(id, "tools.call needs a tool \"name\" in params"), true) };
            let arguments = req.params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let roots = match parse_roots(req.params.get("roots")) {
                Ok(r) => Arc::new(r),
                Err(e) => return (fail(id, e), true),
            };
            let (reply, rx) = channel();
            let request = Request { name: name.to_string(), arguments, roots, deadline: Instant::now() + REQUEST_TIMEOUT, reply };
            if tx.send(request).is_err() {
                return (fail(id, "AniCraft is closing"), false);
            }
            wake();
            match rx.recv_timeout(REQUEST_TIMEOUT + GRACE) {
                Ok(Ok(result)) => (ok(id, result), true),
                Ok(Err(e)) => (fail(id, e), true),
                Err(RecvTimeoutError::Timeout) => (fail(id, "AniCraft didn't answer in time; the call may still have run. Check with anicraft_document before trying again."), true),
                Err(RecvTimeoutError::Disconnected) => (fail(id, "AniCraft closed before answering"), false),
            }
        }
        other => (fail(id, format!("unknown method '{other}' (hello, tools.list, tools.call)")), true),
    }
}

// ----- the client: `anicraft-cli mcp --bridge` -------------------------------------------------

/// Reads `--bridge`'s value: a port, or `127.0.0.1:port` / `localhost:port`. Other hosts are
/// refused: the control port is for this computer only.
pub fn parse_bridge_target(s: &str) -> Result<SocketAddr, String> {
    let s = s.trim();
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h, p),
        None => ("127.0.0.1", s),
    };
    if !matches!(host, "127.0.0.1" | "localhost") {
        return Err(format!("--bridge connects to this computer only: give a port, or 127.0.0.1:<port> (not {host})"));
    }
    Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, parse_port(port)?)))
}

/// The token to send: `ANICRAFT_CONTROL_TOKEN` when set, else the one in `control.json`.
pub fn find_token(env: Option<String>, file: Option<&Path>) -> Result<String, String> {
    if let Some(t) = env {
        return Ok(t);
    }
    let Some(file) = file else { return Err(not_running(None)) };
    match std::fs::read_to_string(file) {
        Ok(text) => ControlFile::parse(&text).map(|c| c.token),
        Err(_) => Err(not_running(None)),
    }
}

/// What to tell the person when the app isn't there.
fn not_running(port: Option<u16>) -> String {
    let port = port.unwrap_or(DEFAULT_PORT);
    format!(
        "AniCraft isn't open with remote control on. Start it with `anicraft --control {port}` (the \"AniCraft (Claude)\" shortcut), then try again; there is no need to restart the assistant."
    )
}

/// Why a bridged call failed.
#[derive(Debug, PartialEq)]
pub enum BridgeError {
    /// Couldn't reach the app (not running, no token): nothing ran.
    NotRunning(String),
    /// The connection broke after the request went out: it may have run.
    Lost(String),
    /// The app refused the call (the headless server's protocol error).
    Refused(String),
}

struct Conn {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// The app at the other end applies automation roots (its `hello` said protocol 2).
    applies_roots: bool,
}

impl Conn {
    /// Whether the app is still at the other end (an app that quit leaves the socket closed).
    fn alive(&self) -> bool {
        if self.writer.set_nonblocking(true).is_err() {
            return false;
        }
        let r = self.writer.peek(&mut [0u8; 1]);
        let back = self.writer.set_nonblocking(false).is_ok();
        back && matches!(r, Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock)
    }
}

/// The bridge's link to the app: connected on the first call, again after a break.
pub struct Bridge {
    addr: SocketAddr,
    conn: Option<Conn>,
    next_id: u64,
    /// Where the token comes from (tests point it elsewhere).
    token_file: Option<PathBuf>,
    /// The automation roots sent with every call (`null`: none, so the app allows no file).
    roots: Value,
}

impl Bridge {
    /// Doesn't connect: the assistant starts its servers when it starts, often before AniCraft.
    pub fn new(addr: SocketAddr) -> Bridge {
        Bridge { addr, conn: None, next_id: 1, token_file: control_file(), roots: Value::Null }
    }

    pub fn with_token_file(addr: SocketAddr, file: Option<PathBuf>) -> Bridge {
        Bridge { addr, conn: None, next_id: 1, token_file: file, roots: Value::Null }
    }

    /// The folders this assistant's commands may use, sent with every call for the app to
    /// apply (ADR 0124). They go as text, so a folder whose name is not Unicode is refused.
    pub fn with_roots(mut self, roots: &Roots) -> Result<Bridge, String> {
        let text = |p: &Path| p.to_str().map(str::to_string).ok_or_else(|| format!("{}: an automation root's name must be Unicode text", p.display()));
        let read = roots.read().iter().map(|p| text(p)).collect::<Result<Vec<_>, _>>()?;
        let write = roots.write().map(text).transpose()?;
        self.roots = json!({"read": read, "write": write});
        Ok(self)
    }

    fn connect(&mut self) -> Result<String, BridgeError> {
        let token = find_token(token_from_env(), self.token_file.as_deref()).map_err(BridgeError::NotRunning)?;
        if self.conn.as_ref().is_some_and(Conn::alive) {
            return Ok(token);
        }
        self.conn = None;
        let stream = TcpStream::connect_timeout(&self.addr, Duration::from_secs(3)).map_err(|_| BridgeError::NotRunning(not_running(Some(self.addr.port()))))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT + GRACE + Duration::from_secs(5)));
        let reader = BufReader::new(stream.try_clone().map_err(|e| BridgeError::NotRunning(e.to_string()))?);
        self.conn = Some(Conn { reader, writer: stream, applies_roots: false });
        Ok(token)
    }

    /// One round trip; any transport failure drops the connection.
    fn round_trip(&mut self, token: &str, method: &str, params: Value) -> Result<Value, BridgeError> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"id": id, "token": token, "method": method, "params": params}).to_string();
        let Some(conn) = self.conn.as_mut() else { return Err(BridgeError::NotRunning(not_running(Some(self.addr.port())))) };
        let sent = writeln!(conn.writer, "{line}").and_then(|()| conn.writer.flush());
        let reply = sent.map_err(|e| e.to_string()).and_then(|()| match read_line(&mut conn.reader, MAX_LINE) {
            Ok(Some(l)) => Ok(l),
            Ok(None) => Err("AniCraft closed the connection".to_string()),
            Err(LineError::Io(e)) => Err(e.to_string()),
            Err(_) => Err("unreadable reply".to_string()),
        });
        let reply = match reply {
            Ok(l) => l,
            Err(e) => {
                self.conn = None;
                return Err(BridgeError::Lost(e));
            }
        };
        let v: Value = serde_json::from_str(&reply).map_err(|e| {
            self.conn = None;
            BridgeError::Lost(format!("unreadable reply: {e}"))
        })?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        let error = v.get("error").and_then(Value::as_str).unwrap_or("AniCraft refused the request").to_string();
        if v.get("id").is_some_and(Value::is_null) || error.starts_with("missing token") || error.starts_with("wrong token") {
            // The app closes the connection after these (an old token: AniCraft was restarted
            // and control.json has the new one by the next call).
            self.conn = None;
        }
        Err(BridgeError::Refused(error))
    }

    /// One tool call, once the app has said it applies automation roots: an older AniCraft
    /// would ignore them and run the command anywhere, so it is not sent the call at all.
    fn checked_call(&mut self, token: &str, name: &str, args: &Value) -> Result<Value, BridgeError> {
        if !self.conn.as_ref().is_some_and(|c| c.applies_roots) {
            let hello = self.round_trip(token, "hello", json!({}))?;
            if hello.get("protocol").and_then(Value::as_u64).unwrap_or(0) < 2 {
                self.conn = None;
                return Err(BridgeError::NotRunning("The AniCraft window that is open is an older version, which can't keep an assistant inside its automation roots. Update AniCraft (Help > Check for Updates), start it again, then try again.".into()));
            }
            if let Some(c) = self.conn.as_mut() {
                c.applies_roots = true;
            }
        }
        self.round_trip(token, "tools.call", json!({"name": name, "arguments": args, "roots": self.roots}))
    }

    /// Runs a tool in the app. An edit whose answer was lost is never sent again (the app may
    /// have made it); a call that only reads is tried once more on a new connection.
    pub fn call_tool(&mut self, name: &str, args: &Value) -> Result<Value, BridgeError> {
        let retries = if mcp::tool_changes_document(name) { 0 } else { 1 };
        let mut attempt = 0;
        loop {
            let token = self.connect()?;
            match self.checked_call(&token, name, args) {
                Err(BridgeError::Lost(_)) if attempt < retries => attempt += 1,
                Err(BridgeError::Refused(e)) if e.starts_with("wrong token") && attempt < 1 => attempt += 1,
                other => return other,
            }
        }
    }
}

/// The bridge's instructions: the same tools, acting on the person's open window.
pub const BRIDGE_INSTRUCTIONS: &str = "AniCraft is a 2D animation studio, and these tools act on the AniCraft window the person \
has open: every edit appears there immediately and can be undone with Ctrl+Z (or edit.undo). Every edit is a command: call \
anicraft_commands to find one (filter by words such as 'tween' or 'export'), then anicraft_run with its id and params, or \
anicraft_run_script for several in order. Frames in params are 0-based. Commands act on the current scene, layer, frame and \
selection, which anicraft_document reports (the person may change them between your calls). Use anicraft_render to see a \
frame. Commands that would lose work or replace a file refuse until params include \"confirm\": true; ask the person before \
confirming. Files can be read only inside the automation read root and written only inside the automation write root the \
person gave this server; an error names them. If AniCraft isn't open with remote control on, the tools say how to start it.";

impl mcp::Backend for Bridge {
    fn instructions(&self) -> &'static str {
        BRIDGE_INSTRUCTIONS
    }
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match self.call_tool(name, args) {
            Ok(v) => Ok(v),
            Err(BridgeError::Refused(e)) => Err(e),
            Err(BridgeError::NotRunning(e)) => Ok(mcp::text_result(&Value::String(e), true)),
            Err(BridgeError::Lost(e)) => {
                let what = if mcp::tool_changes_document(name) { " It may have been made: check with anicraft_document before trying again." } else { "" };
                Ok(mcp::text_result(&Value::String(format!("The connection to AniCraft broke ({e}).{what}")), true))
            }
        }
    }
}

#[cfg(test)]
mod tests;
