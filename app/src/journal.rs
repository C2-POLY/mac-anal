//! The journal of what mac-anal did on the Mac: each action with its origin, what it touched, a
//! measure before and after, and how to put it back. Kept in
//! `~/Library/Application Support/mac-anal/journal.json`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::actions::{self, Action, Done, Env, Prepared, Undo};
use crate::overview::format_bytes;

/// The entries kept: the oldest go first.
const KEEP: usize = 500;

/// Bumped on every write, so a view knows when to read the journal again.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// One writer at a time: the window, the menu bar and the scheduled tasks share the file.
static WRITE: Mutex<()> = Mutex::new(());

/// Who asked for an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The user, from the assistant's plan.
    Assistant,
    /// The user, from a quick action of the menu bar.
    MenuBar,
    /// The user, running a script the assistant wrote.
    Script,
    /// A scheduled task, run by launchd.
    Scheduled,
    /// A client of `mac-anal mcp`, such as Claude Desktop or Claude Code.
    Mcp,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Assistant => "Assistant",
            Origin::MenuBar => "Barre des menus",
            Origin::Script => "Script",
            Origin::Scheduled => "Tâche planifiée",
            Origin::Mcp => "MCP",
        }
    }
}

/// What can be measured around an action: the free space of the home volume, and the state of
/// what the action touches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measure {
    pub free_disk: Option<u64>,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// Seconds since 1970.
    pub at: u64,
    pub origin: Origin,
    pub title: String,
    /// The catalog action, or none for a script.
    #[serde(default)]
    pub action: Option<Action>,
    /// The script that ran, for a script entry.
    #[serde(default)]
    pub script: Option<String>,
    pub before: Measure,
    pub after: Measure,
    #[serde(default)]
    pub undo: Option<Undo>,
    #[serde(default)]
    pub undone: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl Entry {
    /// The effect, in one line: what changed between the two measures.
    pub fn effect(&self) -> String {
        if let Some(e) = &self.error {
            return format!("Échec : {e}");
        }
        let mut parts = vec![format!("avant : {}, après : {}", self.before.target, self.after.target)];
        if let (Some(b), Some(a)) = (self.before.free_disk, self.after.free_disk) {
            let gained = a as i128 - b as i128;
            let size = format_bytes(gained.unsigned_abs() as u64);
            parts.push(if gained >= 0 { format!("espace libre +{size}") } else { format!("espace libre -{size}") });
            if matches!(self.action, Some(Action::ClearCache { .. })) {
                parts.push("la Corbeille garde la place tant qu'elle n'est pas vidée".into());
            }
        }
        if self.undone {
            parts.push("annulé".into());
        }
        parts.join(" ; ")
    }
}

pub fn path(env: &Env) -> PathBuf {
    env.home.join("Library/Application Support/mac-anal/journal.json")
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The entries, oldest first. A file that cannot be read is kept aside as `.broken`, not lost.
pub fn load(env: &Env) -> Vec<Entry> {
    let path = path(env);
    let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
    match serde_json::from_slice(&bytes) {
        Ok(entries) => entries,
        Err(_) => {
            let _ = std::fs::rename(&path, path.with_extension("json.broken"));
            Vec::new()
        }
    }
}

fn save(path: &Path, entries: &[Entry]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(entries).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{} : {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} : {e}", path.display()))?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Runs an action and writes it to the journal, with its measures before and after, even when it
/// fails. The returned `Done` carries the entry's id, to undo it from anywhere.
pub fn run(env: &Env, origin: Origin, p: &Prepared, confirmed: bool) -> Result<Done, String> {
    let before = measure(env, &p.action);
    let result = actions::execute(env, p, confirmed);
    if let Action::QuitProcess { pid } = p.action {
        if result.is_ok() {
            wait_for_exit(pid);
        }
    }
    let after = measure(env, &p.action);
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load(env);
    let id = entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    let (undo, error) = match &result {
        Ok(done) => (done.undo.clone(), None),
        Err(e) => (None, Some(e.clone())),
    };
    entries.push(Entry { id, at: now(), origin, title: p.title.clone(), action: Some(p.action.clone()), script: None, before, after, undo, undone: false, error });
    let excess = entries.len().saturating_sub(KEEP);
    entries.drain(..excess);
    let saved = save(&path(env), &entries);
    let mut done = result?;
    if let Err(e) = saved {
        eprintln!("journal: {e}");
    }
    done.entry = Some(id);
    Ok(done)
}

/// Writes a real script run to the journal: scripts cannot be undone by mac-anal.
pub fn record_script(env: &Env, origin: Origin, purpose: &str, script: &str, run: &crate::scripts::Run) {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load(env);
    let id = entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    let after = Measure { free_disk: free_space(&env.home), target: match (run.timed_out, run.status) {
        (true, _) => "arrêté au bout de 60 s".into(),
        (false, Some(0)) => "terminé sans erreur".into(),
        (false, Some(c)) => format!("code de sortie {c}"),
        (false, None) => "interrompu".into(),
    } };
    let before = Measure { free_disk: None, target: "script lancé".into() };
    entries.push(Entry { id, at: now(), origin, title: purpose.to_string(), action: None, script: Some(script.to_string()), before, after, undo: None, undone: false, error: None });
    let excess = entries.len().saturating_sub(KEEP);
    entries.drain(..excess);
    if let Err(e) = save(&path(env), &entries) {
        eprintln!("journal: {e}");
    }
}

/// Puts back what an entry did, and marks it undone.
pub fn undo(env: &Env, id: u64) -> Result<(), String> {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load(env);
    let entry = entries.iter_mut().find(|e| e.id == id).ok_or("Cette entrée n'est plus dans le journal.")?;
    if entry.undone {
        return Err("Cette action est déjà annulée.".into());
    }
    let u = entry.undo.clone().ok_or("Cette action ne peut pas être annulée.")?;
    actions::undo(&u)?;
    entry.undone = true;
    save(&path(env), &entries)
}

/// The last entries in a few lines, for the assistant to weigh its proposals.
pub fn summary(env: &Env, n: usize) -> String {
    let entries = load(env);
    if entries.is_empty() {
        return "- (nothing yet)\n".into();
    }
    entries.iter().rev().take(n).map(|e| format!("- {} ({}): {}\n", e.title, date(e.at), e.effect())).collect()
}

/// `09/11 18:42`, in the Mac's time zone.
pub fn date(at: u64) -> String {
    let t = at as libc::time_t;
    // SAFETY: localtime_r only writes into the tm we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return at.to_string();
    }
    format!("{:02}/{:02} {:02}:{:02}", tm.tm_mday, tm.tm_mon + 1, tm.tm_hour, tm.tm_min)
}

fn measure(env: &Env, action: &Action) -> Measure {
    let target = match action {
        Action::ClearCache { name } => {
            let dir = env.caches().join(name);
            if dir.is_dir() {
                format!("dossier présent, {}", format_bytes(actions::folder_size(&dir).0))
            } else {
                "dossier absent".into()
            }
        }
        Action::QuitProcess { pid } => {
            if process_alive(*pid) {
                "processus en cours".into()
            } else {
                "processus terminé".into()
            }
        }
        Action::TrashOldItems { folder, days } => match env.old_folder(folder) {
            Some(dir) => format!("{} éléments de plus de {days} jours", actions::old_items(&dir, *days).len()),
            None => "dossier inconnu".into(),
        },
        Action::DisableLoginItem { label } => {
            if env.launch_agents().join(format!("{label}.plist")).is_file() {
                "lancé au démarrage".into()
            } else {
                "désactivé".into()
            }
        }
    };
    Measure { free_disk: free_space(&env.home), target }
}

fn free_space(path: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: statvfs only writes into the struct we hand it.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize)
}

fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// A process asked to quit takes a moment: wait up to a second so the measure after is true.
fn wait_for_exit(pid: u32) {
    for _ in 0..10 {
        if !process_alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Invented entries for the screenshot mode (`MAC_ANAL_JOURNAL=1`).
pub fn sample() -> Vec<Entry> {
    const GB: u64 = 1_000_000_000;
    let at = now();
    let entry = |id, ago, origin, title: &str, action, before: (&str, u64), after: (&str, u64), undo: bool, undone| Entry {
        id,
        at: at - ago,
        origin,
        title: title.into(),
        action: Some(action),
        script: None,
        before: Measure { free_disk: Some(before.1), target: before.0.into() },
        after: Measure { free_disk: Some(after.1), target: after.0.into() },
        undo: undo.then(Undo::none),
        undone,
        error: None,
    };
    vec![
        entry(1, 7200, Origin::Assistant, "Désactiver com.example.updater au démarrage", Action::DisableLoginItem { label: "com.example.updater".into() }, ("lancé au démarrage", 182 * GB), ("désactivé", 182 * GB), true, true),
        entry(2, 1800, Origin::Assistant, "Quitter Example Helper", Action::QuitProcess { pid: 4242 }, ("processus en cours", 182 * GB), ("processus terminé", 182 * GB), false, false),
        entry(3, 120, Origin::MenuBar, "Vider le cache com.example.music", Action::ClearCache { name: "com.example.music".into() }, ("dossier présent, 2,3 Go", 182 * GB), ("dossier absent", 182 * GB), true, false),
    ]
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
    fn an_action_is_journaled_with_its_measures_and_undone_from_there() {
        let (_d, env) = home();
        let cache = env.caches().join("com.example.app");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("blob"), vec![0u8; 4096]).unwrap();
        let p = actions::prepare(&env, Action::ClearCache { name: "com.example.app".into() }).unwrap();

        let g = generation();
        let done = run(&env, Origin::Assistant, &p, false).unwrap();
        assert!(generation() > g);
        let entries = load(&env);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!((e.id, e.origin), (done.entry.unwrap(), Origin::Assistant));
        assert_eq!(e.before.target, "dossier présent, 4,1 Ko");
        assert_eq!(e.after.target, "dossier absent");
        assert!(e.before.free_disk.is_some());
        assert!(e.effect().contains("Corbeille"), "{}", e.effect());

        undo(&env, e.id).unwrap();
        assert!(cache.join("blob").is_file());
        assert!(load(&env)[0].undone);
        assert!(undo(&env, e.id).is_err(), "undone twice");
        assert!(summary(&env, 5).contains("annulé"));
    }

    #[test]
    fn a_failure_is_journaled_too() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("c")).unwrap();
        let p = actions::prepare(&env, Action::ClearCache { name: "c".into() }).unwrap();
        std::fs::remove_dir(env.caches().join("c")).unwrap();
        assert!(run(&env, Origin::MenuBar, &p, false).is_err());
        let e = &load(&env)[0];
        assert!(e.error.is_some() && e.undo.is_none());
        assert!(e.effect().starts_with("Échec"));
    }

    #[test]
    fn a_quit_process_is_measured_after_it_exits() {
        let (_d, env) = home();
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let p = actions::prepare(&env, Action::QuitProcess { pid: child.id() }).unwrap();
        let reaper = std::thread::spawn(move || child.wait());
        run(&env, Origin::Assistant, &p, true).unwrap();
        reaper.join().unwrap().unwrap();
        let e = &load(&env)[0];
        assert_eq!(e.before.target, "processus en cours");
        assert_eq!(e.after.target, "processus terminé");
        assert!(undo(&env, e.id).is_err());
    }

    #[test]
    fn a_broken_journal_is_kept_aside() {
        let (_d, env) = home();
        std::fs::create_dir_all(path(&env).parent().unwrap()).unwrap();
        std::fs::write(path(&env), "[{ broken").unwrap();
        assert!(load(&env).is_empty());
        assert!(path(&env).with_extension("json.broken").is_file());
        assert_eq!(summary(&env, 3), "- (nothing yet)\n");
    }

    #[test]
    fn dates_are_short() {
        let d = date(0);
        assert_eq!(d.len(), "01/01 00:00".len(), "{d}");
    }
}
