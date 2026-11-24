//! Remote control (ADR 0104, `docs/control-protocol.md`): `anicraft --control <port>` opens a
//! control port on this computer, and an assistant (through `anicraft-cli mcp --bridge`) edits
//! the open document live. The port, its threads and the tools are `anicraft_cli::control`'s;
//! this module runs each call on the window's session at the start of a frame, writes the
//! run's token to `control.json` in the settings folder, and removes it on exit.
//!
//! A call that edits waits while the person is dragging (or a plug-in dialog has its preview
//! open), so the two never share an undo step. Typing in a field doesn't hold it back: the
//! person's typed change is committed as its own step first, as clicking elsewhere would.

use crate::app::{App, Drag};
use crate::platform;
use anicraft_cli::{control, mcp};
use anicraft_engine::Session;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

/// An open control port.
pub struct Remote {
    server: control::Server,
    /// Calls in arrival order; the first one that edits holds the rest back during a drag.
    waiting: VecDeque<control::Request>,
    token: String,
    /// `control.json`, when it could be written.
    file: Option<PathBuf>,
}

/// Opens the control port; failures go to the status bar (the window still opens).
pub fn start(app: &mut App, ctx: &egui::Context, port: u16) {
    // In the settings folder, which a portable install keeps beside the program (ADR 0094):
    // nothing is written outside it.
    start_at(app, ctx, port, platform::config_dir().map(|d| d.join(control::CONTROL_FILE)));
}

/// [`start`], with where `control.json` goes.
fn start_at(app: &mut App, ctx: &egui::Context, port: u16, file: Option<PathBuf>) {
    let token = match control::token_from_env().map_or_else(control::new_token, Ok) {
        Ok(t) => t,
        Err(e) => {
            app.status_err(tr!("Remote control is off: {reason}", reason = e));
            return;
        }
    };
    let wake_ctx = ctx.clone();
    let server = match control::listen(port, token.clone(), Arc::new(move || wake_ctx.request_repaint())) {
        Ok(s) => s,
        Err(e) => {
            app.status_err(tr!("Remote control is off: port {port} is in use or not allowed ({reason}). Is another AniCraft already open with remote control?", port = port, reason = e));
            return;
        }
    };
    let written = file.as_ref().map(|path| {
        let contents = control::ControlFile { port: server.port, token: token.clone(), pid: std::process::id(), version: env!("CARGO_PKG_VERSION").to_string() };
        platform::write_private_file(path, contents.to_json().to_string().as_bytes())
    });
    if let Some(Err(e)) = &written {
        // The port works with ANICRAFT_CONTROL_TOKEN; without the file a bridge can't find it.
        app.status_err(tr!("Remote control is on, but its token could not be saved ({reason}); set ANICRAFT_CONTROL_TOKEN for the assistant.", reason = e));
    }
    let file = if matches!(written, Some(Ok(()))) { file } else { None };
    app.remote = Some(Remote { server, waiting: VecDeque::new(), token, file });
}

/// The port, while remote control is on (the status bar shows it).
pub fn port(app: &App) -> Option<u16> {
    app.remote.as_ref().map(|r| r.server.port)
}

/// Whether the person is in the middle of an edit a remote one must not join.
fn person_is_dragging(app: &App, ctx: &egui::Context) -> bool {
    app.drag != Drag::None || app.plugin_dialog.is_some() || app.audio_keys.is_some() || (app.s.in_gesture() && ctx.egui_is_using_pointer())
}

/// Runs the waiting calls: at the start of a frame, so their edits draw in this one.
pub fn poll(app: &mut App, ctx: &egui::Context) {
    let Some(r) = app.remote.as_mut() else { return };
    while let Some(req) = r.server.try_recv() {
        r.waiting.push_back(req);
    }
    loop {
        let dragging = person_is_dragging(app, ctx);
        let Some(r) = app.remote.as_mut() else { return };
        let Some(next) = r.waiting.front() else { return };
        if next.changes_document() && dragging && !next.expired() {
            // Look again shortly: the drag ends without a remote wake-up.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        let Some(req) = r.waiting.pop_front() else { return };
        // (An expired call is refused without running: it leaves the person's gesture alone.)
        if req.changes_document() && !req.expired() && !dragging && app.s.in_gesture() {
            // A typed property change still open: it becomes its own undo step first.
            app.s.end_gesture();
        }
        req.run(&mut Live(app));
    }
}

/// On exit: calls still waiting are told so, and `control.json` goes (if it is still ours).
pub fn finish(app: &mut App) {
    let Some(mut r) = app.remote.take() else { return };
    for req in r.waiting.drain(..) {
        // i18n-exempt: the protocol's English, read by the assistant (as MCP)
        req.refuse("AniCraft is closing");
    }
    if let Some(path) = &r.file {
        let ours = std::fs::read_to_string(path).ok().and_then(|t| control::ControlFile::parse(&t).ok()).is_some_and(|c| c.token == r.token);
        if ours {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The window as the tools' host: commands run with the follow-up the person's own get (the
/// status line, the view a new document resets, Open Recent).
struct Live<'a>(&'a mut App);

impl mcp::Host for Live<'_> {
    fn session(&self) -> &Session {
        &self.0.s
    }
    fn execute(&mut self, id: &str, params: &Value) -> Result<Value, String> {
        let selected = self.0.s.selection.len();
        let revision = self.0.s.revision;
        let v = self.0.s.execute(id, params).map_err(|e| e.to_string())?;
        // An edit to the document behind the start screen: show the document, so the person
        // sees it land (ADR 0059's start screen otherwise covers the workspace).
        if self.0.s.revision != revision {
            self.0.start = None;
        }
        // The window's own follow-up (its settings, Open Recent) is not the assistant's file
        // access: the automation roots apply to the command above only (ADR 0124).
        anicraft_engine::access::unconfined(|| self.0.announce(id, &v, selected));
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use crate::platform;
    use std::path::PathBuf;

    /// The whole path an assistant's edit takes: the port opens and writes its token file, a
    /// bridge finds the token there and runs a command, the frame loop makes the edit on the
    /// window's document, and closing removes the file and frees the port.
    #[test]
    fn an_edit_through_the_port_lands_in_the_window_and_closing_cleans_up() {
        let dir = std::env::temp_dir().join(format!("anicraft-remote-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("control.json");
        // A file left by a run that crashed is replaced, not written through.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "stale").unwrap();
        let ctx = egui::Context::default();
        let mut app = crate::app::App::with_context(&ctx, anicraft_engine::Session::default(), Some(PathBuf::from("unused.png")));
        assert_eq!(super::port(&app), None);
        super::start_at(&mut app, &ctx, 0, Some(file.clone()));
        let port = super::port(&app).expect("the port is open");
        let written = anicraft_cli::control::ControlFile::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((written.port, written.token.len(), written.pid), (port, 64, std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600, "only the owner reads the token");
        }
        // A second window asking for the same port says so and stays without one.
        let mut other = crate::app::App::with_context(&ctx, anicraft_engine::Session::default(), Some(PathBuf::from("unused.png")));
        super::start_at(&mut other, &ctx, port, None);
        assert!(super::port(&other).is_none() && other.status.as_ref().is_some_and(|s| s.error));

        let before = app.s.revision;
        let token_file = file.clone();
        let client = std::thread::spawn(move || {
            let addr = anicraft_cli::control::parse_bridge_target(&port.to_string()).unwrap();
            let mut bridge = anicraft_cli::control::Bridge::with_token_file(addr, Some(token_file));
            let read = bridge.call_tool("anicraft_document", &serde_json::json!({}));
            let edit = bridge.call_tool("anicraft_run", &serde_json::json!({"command": "draw.rect", "params": {"x": 10, "y": 10, "w": 40, "h": 30}}));
            (read.is_ok(), edit)
        });
        while !client.is_finished() {
            super::poll(&mut app, &ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (read, edit) = client.join().unwrap();
        assert!(read, "a call that only reads answers");
        assert!(edit.is_ok(), "{edit:?}");
        assert!(app.s.revision > before, "the rectangle is in the window's document");
        assert!(app.s.history().undo_len() >= 1, "and it undoes like the person's own edit");

        super::finish(&mut app);
        assert!(!file.exists(), "the token file goes when the window closes");
        assert_eq!(super::port(&app), None);
        // The port is free again: the same run can open another window on it (ADR 0092).
        let freed = (0..6000).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
        });
        assert!(freed, "port {port} is still taken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The CLI finds `control.json` with its own copy of the settings-folder rule (it can't use
    /// the shell); they must agree, or the bridge looks in the wrong place.
    #[test]
    fn the_bridge_looks_for_the_token_in_the_settings_folder() {
        assert_eq!(anicraft_cli::control::settings_dir(), platform::config_dir());
        assert_eq!(anicraft_cli::control::control_file(), platform::config_dir().map(|d| d.join("control.json")));
    }
}
