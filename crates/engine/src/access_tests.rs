//! Automation roots (ADR 0124) through real commands: what an agent's `Session::execute` may
//! touch while [`access::confine`] is in force.

use crate::access::{self, NOT_FILES, PATH_PARAMS, Roots};
use crate::{Session, commands};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SECRET: &str = "anicraft-secret-0124";

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `<temp>/…/{work,out,secret}`; `work` is the read root, `out` the write root, and `secret`
/// holds a file no agent may read or replace.
fn scratch(tag: &str) -> (Scratch, Arc<Roots>) {
    let dir = std::env::temp_dir().join(format!("anicraft-roots-{tag}-{}-{}", std::process::id(), crate::vfs::random_suffix()));
    for d in ["work", "out", "secret"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::write(dir.join("secret").join("s.json"), format!("{{\"steps\": [], \"note\": \"{SECRET}\"}}")).unwrap();
    let roots = Roots::new(&[dir.join("work")], Some(&dir.join("out"))).unwrap();
    (Scratch(dir), Arc::new(roots))
}

fn text(p: &Path) -> String {
    p.display().to_string()
}

/// Every file under `dir` with its bytes, in path order.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.push((p.clone(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// A command with a file parameter can't be added without saying what it does there.
#[test]
fn every_command_with_a_path_parameter_is_listed() {
    let word = |doc: &str, w: &str| doc.match_indices(w).any(|(i, _)| {
        let before = doc[..i].chars().next_back();
        let after = doc[i + w.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_') && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
    });
    let all = commands();
    let mut unlisted = Vec::new();
    let mut needless = Vec::new();
    for c in &all {
        let names_one = ["path", "paths", "dir", "folder", "data_home", "appimage"].iter().any(|w| word(c.params, w));
        let listed = PATH_PARAMS.iter().any(|(id, _)| *id == c.id) || NOT_FILES.contains(&c.id);
        if names_one && !listed {
            unlisted.push(c.id);
        }
        if !names_one && NOT_FILES.contains(&c.id) {
            needless.push(c.id);
        }
    }
    assert!(unlisted.is_empty(), "these commands name a path, folder or file: add each to access::PATH_PARAMS, or to NOT_FILES if it is not a place on disk: {unlisted:?}");
    assert!(needless.is_empty(), "NOT_FILES lists commands that name no path: {needless:?}");
    for (id, keys) in PATH_PARAMS {
        let spec = all.iter().find(|c| c.id == *id).unwrap_or_else(|| panic!("{id} is not a command"));
        for (key, _) in *keys {
            assert!(word(spec.params, key), "{id} has no '{key}' parameter");
        }
    }
}

#[test]
fn commands_read_and_write_inside_the_roots_only() {
    let (s, roots) = scratch("cmds");
    let (work, out, secret) = (s.0.join("work"), s.0.join("out"), s.0.join("secret").join("s.json"));
    let mut session = Session::default();
    session.execute("draw.rect", &json!({"x": 1, "y": 1, "w": 9, "h": 9})).unwrap();
    // Put there by the person, before any agent: a document in the read root.
    session.execute("file.save_as", &json!({"path": text(&work.join("a.anicraft"))})).unwrap();

    let _g = access::confine(roots.clone());
    // Writing: the write root, and nowhere else (the read root included).
    session.execute("file.save_as", &json!({"path": text(&out.join("b.anicraft"))})).unwrap();
    let e = session.execute("file.save_as", &json!({"path": text(&work.join("c.anicraft"))})).unwrap_err().to_string();
    assert!(e.contains("param 'path'") && e.contains("automation write root") && e.contains(&access::display(roots.write().unwrap())), "{e}");
    assert!(!work.join("c.anicraft").exists());
    // Reading: the read root. What was just written to the write root is not readable.
    session.execute("file.open", &json!({"path": text(&work.join("a.anicraft")), "confirm": true})).unwrap();
    assert!(session.execute("file.open", &json!({"path": text(&out.join("b.anicraft")), "confirm": true})).is_err());
    let e = session.execute("file.open", &json!({"path": text(&secret), "confirm": true})).unwrap_err().to_string();
    assert!(e.contains("automation read root"), "{e}");
    // `..` from inside a root, and a relative path (the working folder is not a root).
    assert!(session.execute("file.open", &json!({"path": text(&work.join("..").join("secret").join("s.json")), "confirm": true})).is_err());
    assert!(session.execute("file.export_png", &json!({"path": "frame.png"})).is_err());
    assert!(session.execute("file.export_png", &json!({"path": text(&out.join("..").join("secret").join("f.png"))})).is_err());
    // A root itself is inside: a sequence straight into the write root.
    session.execute("file.export_png_sequence", &json!({"dir": text(&out), "from": 0, "to": 0})).unwrap();
    // A path the command works out itself is checked where the file is touched: the opened
    // document's own file is in the read root, so a plain Save is refused.
    session.execute("draw.rect", &json!({"x": 20, "y": 1, "w": 9, "h": 9})).unwrap();
    let e = session.execute("file.save", &json!({})).unwrap_err().to_string();
    assert!(e.contains("automation write root"), "{e}");
    // Replacing a file in the write root still asks first (ADR 0004): the command can see it.
    let e = session.execute("file.export_svg", &json!({"path": text(&out.join("b.anicraft"))})).unwrap_err();
    assert!(matches!(e, crate::EngineError::NeedsConfirmation(_)), "{e}");
    // A saved command's steps are commands too.
    let steps = json!({"steps": [{"cmd": "file.save_as", "params": {"path": text(&s.0.join("secret").join("x.anicraft")), "confirm": true}}]});
    assert!(session.execute("macro.run", &steps).is_err());
    assert!(!s.0.join("secret").join("x.anicraft").exists());
    // Reading a saved command from outside: refused, and its text is not in the error.
    let e = session.execute("macro.run", &json!({"path": text(&secret)})).unwrap_err().to_string();
    assert!(e.contains("automation read root") && !e.contains(SECRET), "{e}");
    drop(_g);
    // The person's own commands are not limited.
    session.execute("file.save", &json!({})).unwrap();
}

/// The command fuzz with roots set: every command, every file parameter aimed outside the
/// roots (and then inside them), must neither panic nor touch or reveal a file outside.
#[test]
fn panic_hunt_every_command_with_roots_set() {
    let (s, roots) = scratch("fuzz");
    let (work, out, secret_dir) = (s.0.join("work"), s.0.join("out"), s.0.join("secret"));
    let secret = secret_dir.join("s.json");
    std::fs::write(work.join("in.json"), b"{\"steps\": []}").unwrap();
    let before = snapshot(&secret_dir);
    let aim = |file: &Path, new: &Path, dir: &Path| {
        json!({
            "path": text(file), "paths": [text(file), text(new)], "dir": text(dir), "folder": text(dir), "data_home": text(dir), "appimage": text(file),
            "output": text(new), "name": "x", "to": "y", "session": "s1", "category": "c", "format": "nope", "template": Value::Null,
            "confirm": true, "from": 0, "to_frame": 0, "index": 0, "steps": [{"cmd": "file.save_as", "params": {"path": text(new), "confirm": true}}],
        })
    };
    let outside = [
        aim(&secret, &secret_dir.join("new.png"), &secret_dir),
        aim(&work.join("..").join("secret").join("s.json"), &out.join("..").join("secret").join("new2.png"), &out.join("..").join("secret")),
        aim(&secret_dir.join("new3.anicraft"), &secret, &secret_dir.join("made")),
        json!({"path": text(&secret)}),
        json!({"path": text(&secret_dir.join("new4.json")), "confirm": true}),
        json!({"folder": text(&secret_dir), "name": "s", "confirm": true}),
        json!({"dir": text(&secret_dir), "confirm": true}),
    ];
    let inside = [aim(&work.join("in.json"), &out.join("new.png"), &out), aim(&work.join("in.json"), &out.join("new.json"), &work), json!({"path": text(&out.join("p.json")), "confirm": true})];
    let _g = access::confine(roots);
    let mut said = Vec::new();
    for c in commands() {
        for (i, p) in outside.iter().chain(&inside).enumerate() {
            let mut session = Session::default();
            let _ = session.execute("draw.rect", &json!({"x": 1, "y": 1, "w": 9, "h": 9}));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.execute(c.id, p)));
            match r {
                Ok(Ok(v)) => said.push(v.to_string()),
                Ok(Err(e)) => said.push(e.to_string()),
                Err(_) => panic!("{} panicked on params {i}", c.id),
            }
        }
    }
    drop(_g);
    assert_eq!(snapshot(&secret_dir), before, "nothing outside the roots was made, changed or removed");
    assert!(!secret_dir.join("made").exists());
    assert!(said.iter().all(|t| !t.contains(SECRET)), "no result or error holds a refused file's text");
    assert!(snapshot(&out).len() > 3, "the write root was written to");
}
