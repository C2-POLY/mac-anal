//! The closed set of actions the assistant may propose. The AI only picks and fills them; the app
//! validates each one, shows what it touches, runs it and undoes it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

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

/// An action as the AI writes it, before validation.
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

/// The kinds the AI may use, with what each takes, for its instructions and its JSON schema.
pub const CATALOG: &[(&str, &str)] = &[
    ("clear_cache", "name: a folder directly under ~/Library/Caches (Trash, can be undone)"),
    ("quit_process", "pid: a process of the user (asks it to quit; unsaved work may be lost)"),
    ("disable_login_item", "label: a user LaunchAgent, ~/Library/LaunchAgents/<label>.plist (set aside, can be undone)"),
];

/// Where the actions read and write: the user's home, replaced by a temporary folder in tests.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
}

impl Env {
    pub fn current() -> Option<Self> {
        std::env::var_os("HOME").map(|h| Env { home: PathBuf::from(h) })
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
}

/// What an action did, and how to put it back when it can be.
#[derive(Clone, Debug)]
pub struct Done {
    pub summary: String,
    pub undo: Option<Undo>,
}

/// Files moved by an action, to move back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undo {
    moves: Vec<(PathBuf, PathBuf)>,
}

/// Checks an action against the Mac and describes what it will touch. Refuses anything outside its
/// folder, a process that is not the user's, mac-anal itself or launchd.
pub fn prepare(env: &Env, action: Action) -> Result<Prepared, String> {
    match &action {
        Action::ClearCache { name } => {
            plain_name(name)?;
            let dir = env.caches().join(name);
            if !dir.is_dir() {
                return Err(format!("Le cache « {name} » n'existe pas."));
            }
            let (bytes, files) = folder_size(&dir);
            Ok(Prepared {
                title: format!("Vider le cache {name}"),
                preview: format!(
                    "Envoie {} à la Corbeille ({} fichiers, {}).",
                    dir.display(),
                    files,
                    crate::overview::format_bytes(bytes)
                ),
                level: Level::Reversible,
                action,
            })
        }
        Action::QuitProcess { pid } => {
            let pid = *pid;
            if pid <= 1 || pid == std::process::id() {
                return Err(format!("Le processus {pid} ne peut pas être quitté par mac-anal."));
            }
            let (user, name) = process_owner_and_name(pid).ok_or_else(|| format!("Aucun processus {pid}."))?;
            if Some(user.as_str()) != current_user().as_deref() {
                return Err(format!("Le processus {pid} ({name}) appartient à {user}, pas à vous."));
            }
            Ok(Prepared {
                title: format!("Quitter {name}"),
                preview: format!("Demande au processus {pid} ({name}) de quitter. Le travail non enregistré peut être perdu."),
                level: Level::Irreversible,
                action,
            })
        }
        Action::DisableLoginItem { label } => {
            plain_name(label)?;
            let plist = env.launch_agents().join(format!("{label}.plist"));
            if !plist.is_file() {
                return Err(format!("Aucun élément de démarrage « {label} »."));
            }
            Ok(Prepared {
                title: format!("Désactiver {label} au démarrage"),
                preview: format!(
                    "Met de côté {} ; il ne se lancera plus à la prochaine ouverture de session.",
                    plist.display()
                ),
                level: Level::Reversible,
                action,
            })
        }
    }
}

/// Runs a prepared action. An irreversible one runs only once `confirmed` is true.
pub fn execute(env: &Env, prepared: &Prepared, confirmed: bool) -> Result<Done, String> {
    if prepared.level == Level::Irreversible && !confirmed {
        return Err("Cette action demande une confirmation.".into());
    }
    match &prepared.action {
        Action::ClearCache { name } => {
            let from = env.caches().join(name);
            let to = free_path(&env.trash(), name);
            move_to(&from, &to)?;
            Ok(Done { summary: format!("{name} est dans la Corbeille."), undo: Some(Undo { moves: vec![(from, to)] }) })
        }
        Action::QuitProcess { pid } => {
            // The PID may have been reused since the plan: check it is still the same process of the user.
            if prepare(env, prepared.action.clone()).map(|now| now.title) != Ok(prepared.title.clone()) {
                return Err(format!("Le processus {pid} n'est plus celui proposé : rien n'a été fait."));
            }
            // SAFETY: kill only sends a signal; prepare() refused launchd, mac-anal and other users' processes.
            let r = unsafe { libc::kill(*pid as libc::pid_t, libc::SIGTERM) };
            if r != 0 {
                return Err(format!("Le processus {pid} n'a pas pu être quitté : {}", std::io::Error::last_os_error()));
            }
            Ok(Done { summary: format!("Le processus {pid} a reçu la demande de quitter."), undo: None })
        }
        Action::DisableLoginItem { label } => {
            let file = format!("{label}.plist");
            let from = env.launch_agents().join(&file);
            let to = free_path(&env.disabled_items(), &file);
            move_to(&from, &to)?;
            Ok(Done { summary: format!("{label} ne se lancera plus au démarrage."), undo: Some(Undo { moves: vec![(from, to)] }) })
        }
    }
}

/// Moves back what an action moved. Refuses to overwrite something that took its place since.
pub fn undo(undo: &Undo) -> Result<(), String> {
    for (original, moved) in undo.moves.iter().rev() {
        if original.exists() {
            return Err(format!("{} existe de nouveau : rien n'a été remis.", original.display()));
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
    let mut n = 0;
    loop {
        let candidate = dir.join(if n == 0 { format!("{name} (mac-anal {secs})") } else { format!("{name} (mac-anal {secs}-{n})") });
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
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

fn process_owner_and_name(pid: u32) -> Option<(String, String)> {
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "user=,comm="]).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (user, comm) = line.split_once(char::is_whitespace)?;
    let name = Path::new(comm.trim()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| comm.trim().to_string());
    Some((user.to_string(), name))
}

fn current_user() -> Option<String> {
    std::env::var("USER").ok().filter(|u| !u.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        (dir, env)
    }

    #[test]
    fn unknown_kinds_and_fields_are_refused() {
        assert!(serde_json::from_str::<Action>(r#"{"kind":"rm_rf","path":"/"}"#).is_err());
        assert!(serde_json::from_str::<Action>(r#"{"kind":"clear_cache","name":"x","path":"/"}"#).is_err());
        assert_eq!(
            serde_json::from_str::<Action>(r#"{"kind":"quit_process","pid":42}"#).unwrap(),
            Action::QuitProcess { pid: 42 }
        );
    }

    #[test]
    fn names_cannot_leave_their_folder() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches()).unwrap();
        for bad in ["..", ".", "", "a/b", "../Documents"] {
            assert!(prepare(&env, Action::ClearCache { name: bad.into() }).is_err(), "{bad}");
            assert!(prepare(&env, Action::DisableLoginItem { label: bad.into() }).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_cache_goes_to_the_trash_and_comes_back() {
        let (_d, env) = home();
        let cache = env.caches().join("com.example.app");
        std::fs::create_dir_all(cache.join("sub")).unwrap();
        std::fs::write(cache.join("sub/blob"), vec![0u8; 2048]).unwrap();

        let p = prepare(&env, Action::ClearCache { name: "com.example.app".into() }).unwrap();
        assert_eq!(p.level, Level::Reversible);
        assert!(p.preview.contains("1 fichiers"), "{}", p.preview);

        let done = execute(&env, &p, false).unwrap();
        assert!(!cache.exists());
        assert!(env.trash().join("com.example.app/sub/blob").is_file());

        undo(done.undo.as_ref().unwrap()).unwrap();
        assert!(cache.join("sub/blob").is_file());
        assert!(!env.trash().join("com.example.app").exists());
    }

    #[test]
    fn a_name_already_in_the_trash_is_kept() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("c")).unwrap();
        std::fs::create_dir_all(env.trash().join("c")).unwrap();
        let p = prepare(&env, Action::ClearCache { name: "c".into() }).unwrap();
        execute(&env, &p, false).unwrap();
        assert!(env.trash().join("c").is_dir());
        assert_eq!(std::fs::read_dir(env.trash()).unwrap().count(), 2);
    }

    #[test]
    fn a_login_item_is_set_aside_and_put_back() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.launch_agents()).unwrap();
        let plist = env.launch_agents().join("com.example.agent.plist");
        std::fs::write(&plist, "<plist/>").unwrap();

        let p = prepare(&env, Action::DisableLoginItem { label: "com.example.agent".into() }).unwrap();
        let done = execute(&env, &p, false).unwrap();
        assert!(!plist.exists());
        assert!(env.disabled_items().join("com.example.agent.plist").is_file());

        undo(done.undo.as_ref().unwrap()).unwrap();
        assert!(plist.is_file());
        assert!(prepare(&env, Action::DisableLoginItem { label: "missing".into() }).is_err());
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
    fn quitting_needs_a_confirmation_and_only_reaches_own_processes() {
        let (_d, env) = home();
        assert!(prepare(&env, Action::QuitProcess { pid: 1 }).is_err());
        assert!(prepare(&env, Action::QuitProcess { pid: std::process::id() }).is_err());

        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let p = prepare(&env, Action::QuitProcess { pid: child.id() }).unwrap();
        assert_eq!(p.level, Level::Irreversible);
        assert!(execute(&env, &p, false).is_err());
        assert!(child.try_wait().unwrap().is_none(), "ran without confirmation");

        let mut stale = p.clone();
        stale.title = "Quitter autre-chose".into();
        assert!(execute(&env, &stale, true).is_err(), "a reused PID must be refused");
        assert!(child.try_wait().unwrap().is_none());

        execute(&env, &p, true).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
