//! The closed set of actions the assistant may propose. The AI only picks and fills them; the app
//! validates each one, shows what it touches, runs it and undoes it.

mod cache;
mod login_item;
mod process;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

/// How much an action can change on the Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Changes nothing.
    #[allow(dead_code)] // no read-only action yet; the scripts and the investigation (#2, #5) add some
    Read,
    /// Changes something mac-anal can put back.
    Reversible,
    /// Cannot be put back, or may lose unsaved work: never run without an explicit confirmation.
    Irreversible,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Read => "Lecture",
            Level::Reversible => "Réversible",
            Level::Irreversible => "Irréversible",
        }
    }
}

/// An action as the AI writes it. Any other kind, or any other field, is refused when it is read.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Send a folder directly under `~/Library/Caches` to the Trash.
    ClearCache { name: String },
    /// Ask a process of the user to quit (SIGTERM).
    QuitProcess { pid: u32 },
    /// Set aside `~/Library/LaunchAgents/<label>.plist`, so it no longer starts at login.
    DisableLoginItem { label: String },
}

/// What the one field of an action holds, for the JSON schema.
#[derive(Clone, Copy)]
pub enum Field {
    Text,
    Integer,
}

/// One known action: its `kind`, its level, its field and what it does, for the AI's instructions.
pub struct Spec {
    pub kind: &'static str,
    pub level: Level,
    pub field: &'static str,
    pub field_type: Field,
    pub what: &'static str,
}

/// Every action the AI may choose. `Action` must have one variant per entry (checked by a test).
pub const REGISTRY: &[Spec] = &[
    Spec {
        kind: "clear_cache",
        level: Level::Reversible,
        field: "name",
        field_type: Field::Text,
        what: "Moves the folder ~/Library/Caches/<name> to the Trash (can be undone). `name` is a folder listed in the context.",
    },
    Spec {
        kind: "quit_process",
        level: Level::Irreversible,
        field: "pid",
        field_type: Field::Integer,
        what: "Asks a process of the user to quit; unsaved work may be lost. `pid` is a process listed in the context.",
    },
    Spec {
        kind: "disable_login_item",
        level: Level::Reversible,
        field: "label",
        field_type: Field::Text,
        what: "Sets aside ~/Library/LaunchAgents/<label>.plist so it no longer starts at login (can be undone). `label` is listed in the context.",
    },
];

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Action::ClearCache { .. } => "clear_cache",
            Action::QuitProcess { .. } => "quit_process",
            Action::DisableLoginItem { .. } => "disable_login_item",
        }
    }

    pub fn level(&self) -> Level {
        spec(self.kind()).map_or(Level::Irreversible, |s| s.level)
    }
}

fn spec(kind: &str) -> Option<&'static Spec> {
    REGISTRY.iter().find(|s| s.kind == kind)
}

/// Reads one action written by the AI. An unknown kind or a badly filled one is refused.
pub fn parse(value: &Value) -> Result<Action, String> {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    if spec(kind).is_none() {
        return Err(format!("action inconnue « {kind} »"));
    }
    Action::deserialize(value).map_err(|e| format!("action « {kind} » mal remplie : {e}"))
}

/// The JSON schema of one action: one shape per entry of the registry.
pub fn schema() -> Value {
    let shapes: Vec<Value> = REGISTRY
        .iter()
        .map(|s| {
            let field = match s.field_type {
                Field::Text => json!({"type": "string"}),
                Field::Integer => json!({"type": "integer", "minimum": 2}),
            };
            json!({
                "type": "object",
                "description": s.what,
                "properties": {"kind": {"const": s.kind}, s.field: field},
                "required": ["kind", s.field],
                "additionalProperties": false,
            })
        })
        .collect();
    json!({"anyOf": shapes})
}

/// The registry as lines for the AI's instructions.
pub fn catalog() -> String {
    REGISTRY.iter().map(|s| format!("- {} ({}): {}\n", s.kind, s.field, s.what)).collect()
}

/// Where the actions read and write: the user's home, replaced by a temporary folder in tests.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
}

impl Env {
    pub fn current() -> Option<Self> {
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| Env { home: PathBuf::from(h) })
    }

    pub fn caches(&self) -> PathBuf {
        self.home.join("Library/Caches")
    }

    pub fn launch_agents(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents")
    }

    fn trash(&self) -> PathBuf {
        self.home.join(".Trash")
    }

    /// Where disabled login items wait until they are put back.
    fn disabled_items(&self) -> PathBuf {
        self.home.join("Library/Application Support/mac-anal/disabled-login-items")
    }
}

/// A validated action, with what it will touch.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub action: Action,
    pub level: Level,
    pub title: String,
    pub preview: String,
    /// What the preview was made for (a path, a process name), checked again just before running.
    target: String,
}

impl Prepared {
    /// For the screenshot mode: a preview written by hand, never checked against the Mac.
    pub fn sample(action: Action, title: &str, preview: &str) -> Self {
        Prepared { level: action.level(), action, title: title.into(), preview: preview.into(), target: String::new() }
    }
}

/// What an action did, and how to put it back when it can be.
#[derive(Clone, Debug)]
pub struct Done {
    pub summary: String,
    pub undo: Option<Undo>,
}

impl Done {
    /// For the screenshot mode: done and undoable, with nothing to move back.
    pub fn sample(summary: &str) -> Self {
        Done { summary: summary.into(), undo: Some(Undo { moves: Vec::new() }) }
    }
}

/// Files moved by an action, to move back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undo {
    moves: Vec<(PathBuf, PathBuf)>,
}

/// Checks an action against the Mac and describes what it will touch. Refuses anything outside its
/// folder, a process that is not the user's, mac-anal itself or launchd.
pub fn prepare(env: &Env, action: Action) -> Result<Prepared, String> {
    let level = action.level();
    let (title, preview, target) = match &action {
        Action::ClearCache { name } => cache::prepare(env, name)?,
        Action::QuitProcess { pid } => process::prepare(*pid)?,
        Action::DisableLoginItem { label } => login_item::prepare(env, label)?,
    };
    Ok(Prepared { action, level, title, preview, target })
}

/// Runs a prepared action. An irreversible one runs only once `confirmed` is true.
pub fn execute(env: &Env, prepared: &Prepared, confirmed: bool) -> Result<Done, String> {
    if prepared.level == Level::Irreversible && !confirmed {
        return Err("Cette action demande une confirmation.".into());
    }
    match &prepared.action {
        Action::ClearCache { name } => cache::execute(env, name),
        Action::QuitProcess { pid } => process::execute(*pid, &prepared.target),
        Action::DisableLoginItem { label } => login_item::execute(env, label),
    }
}

/// Moves back what an action moved. Refuses to overwrite something that took its place since.
pub fn undo(undo: &Undo) -> Result<(), String> {
    for (original, moved) in undo.moves.iter().rev() {
        if original.exists() {
            return Err(format!("{} existe de nouveau : rien n'a été remis.", original.display()));
        }
        if !moved.exists() {
            return Err(format!("{} n'existe plus : rien n'a été remis.", moved.display()));
        }
        move_to(moved, original)?;
    }
    Ok(())
}

/// A file or folder name alone: no separator, nothing that climbs out of its folder.
fn plain_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(format!("« {name} » n'est pas un nom valide."));
    }
    Ok(())
}

/// Renames, never copies then deletes: the Trash and the home are on the same volume.
fn move_to(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    }
    std::fs::rename(from, to).map_err(|e| format!("{} → {} : {e}", from.display(), to.display()))
}

/// `dir/name`, or `dir/name (mac-anal <seconds>)` when the name is taken.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let plain = dir.join(name);
    if !plain.exists() {
        return plain;
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (0..)
        .map(|n| dir.join(if n == 0 { format!("{name} (mac-anal {secs})") } else { format!("{name} (mac-anal {secs}-{n})") }))
        .find(|candidate| !candidate.exists())
        .expect("an unbounded range always finds a free name")
}

/// Total size and number of files under a folder, without following links.
pub fn folder_size(dir: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        (dir, env)
    }

    #[test]
    fn unknown_kinds_and_fields_are_refused() {
        assert!(parse(&json!({"kind": "rm_rf", "path": "/"})).unwrap_err().contains("inconnue"));
        assert!(parse(&json!({"path": "/"})).unwrap_err().contains("inconnue"));
        assert!(parse(&json!({"kind": "clear_cache", "name": "x", "path": "/"})).unwrap_err().contains("mal remplie"));
        assert!(parse(&json!({"kind": "quit_process", "pid": "42"})).is_err());
        assert!(parse(&json!({"kind": "quit_process", "pid": -3})).is_err());
        assert_eq!(parse(&json!({"kind": "quit_process", "pid": 42})).unwrap(), Action::QuitProcess { pid: 42 });
    }

    #[test]
    fn the_registry_and_the_enum_agree() {
        let samples = [
            json!({"kind": "clear_cache", "name": "x"}),
            json!({"kind": "quit_process", "pid": 42}),
            json!({"kind": "disable_login_item", "label": "x"}),
        ];
        assert_eq!(samples.len(), REGISTRY.len());
        for (sample, spec) in samples.iter().zip(REGISTRY) {
            let action = parse(sample).unwrap();
            assert_eq!(action.kind(), spec.kind);
            assert!(sample.get(spec.field).is_some(), "{}", spec.kind);
        }
        assert_eq!(schema()["anyOf"].as_array().unwrap().len(), REGISTRY.len());
        assert!(catalog().contains("clear_cache"));
    }

    #[test]
    fn levels_come_from_the_registry() {
        assert_eq!(Action::ClearCache { name: "x".into() }.level(), Level::Reversible);
        assert_eq!(Action::DisableLoginItem { label: "x".into() }.level(), Level::Reversible);
        assert_eq!(Action::QuitProcess { pid: 42 }.level(), Level::Irreversible);
    }

    #[test]
    fn names_cannot_leave_their_folder() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches()).unwrap();
        std::fs::create_dir_all(env.launch_agents()).unwrap();
        for bad in ["..", ".", "", "a/b", "../Documents", "a\0b"] {
            assert!(prepare(&env, Action::ClearCache { name: bad.into() }).is_err(), "{bad}");
            assert!(prepare(&env, Action::DisableLoginItem { label: bad.into() }).is_err(), "{bad}");
        }
    }

    #[test]
    fn undo_never_overwrites() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("c")).unwrap();
        let p = prepare(&env, Action::ClearCache { name: "c".into() }).unwrap();
        let done = execute(&env, &p, false).unwrap();
        std::fs::create_dir_all(env.caches().join("c")).unwrap();
        assert!(undo(done.undo.as_ref().unwrap()).is_err());
        assert!(env.trash().join("c").is_dir());
    }

    #[test]
    fn undo_says_when_the_trash_was_emptied() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("c")).unwrap();
        let p = prepare(&env, Action::ClearCache { name: "c".into() }).unwrap();
        let done = execute(&env, &p, false).unwrap();
        std::fs::remove_dir(env.trash().join("c")).unwrap();
        assert!(undo(done.undo.as_ref().unwrap()).unwrap_err().contains("n'existe plus"));
    }
}
