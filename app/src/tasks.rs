//! Scheduled tasks: a recurring job stated in a sentence, turned by Claude into a time and one
//! thing to run, either a reversible action of the catalog or a read-only script of the library.
//! Each task is a launchd agent that runs `mac-anal run-task <id>` at its time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::actions::{self, Action, Env, Level};
use crate::ai;
use crate::journal::{self, Origin};
use crate::launchd::{self, Agent, Trigger};
use crate::scripts;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static WRITE: Mutex<()> = Mutex::new(());

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

const DAYS: [&str; 7] = ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"];

/// What a task runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    Action(Action),
    /// A script of the library, by name.
    Script(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub name: String,
    /// 0 is Sunday; none is every day.
    pub weekday: Option<u8>,
    pub hour: u8,
    pub minute: u8,
    pub what: What,
    #[serde(default)]
    pub paused: bool,
}

/// How the last run went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    pub at: u64,
    pub ok: bool,
    pub summary: String,
}

impl Task {
    pub fn schedule(&self) -> String {
        let day = self.weekday.and_then(|d| DAYS.get(d as usize)).map_or("chaque jour".to_string(), |d| format!("chaque {d}"));
        format!("{day} à {:02}:{:02}", self.hour, self.minute)
    }

    fn agent(&self) -> Result<Agent, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(Agent {
            name: format!("task-{}", self.id),
            program: vec![exe.display().to_string(), "run-task".into(), self.id.to_string()],
            trigger: Trigger::Calendar { weekday: self.weekday, hour: self.hour, minute: self.minute },
        })
    }

    /// The plist launchd will load, to show before scheduling.
    pub fn plist(&self) -> Result<String, String> {
        self.agent().map(|a| a.plist())
    }

    /// Checks the time and what to run: a reversible action, or a script the app reads as
    /// read-only and without `sudo`. Returns what it will do.
    pub fn check(&self, env: &Env) -> Result<String, String> {
        if self.weekday.is_some_and(|d| d > 6) || self.hour > 23 || self.minute > 59 {
            return Err(format!("Horaire invalide : {}.", self.schedule()));
        }
        match &self.what {
            What::Action(a) => {
                let p = actions::prepare(env, a.clone())?;
                if p.level != Level::Reversible {
                    return Err(format!("« {} » ne peut pas être annulée : une tâche planifiée ne la lancera pas.", p.title));
                }
                Ok(p.preview)
            }
            What::Script(name) => {
                let s = scripts::list(env).into_iter().find(|s| &s.name == name).ok_or_else(|| format!("Aucun script « {name} » dans la bibliothèque."))?;
                let summary = scripts::summarize(&s.script);
                if summary.sudo || summary.level() != Level::Read {
                    return Err(format!("Le script « {} » modifie le Mac : une tâche planifiée ne lance que des scripts en lecture seule.", s.purpose));
                }
                Ok(format!("Lance le script « {} ».", s.purpose))
            }
        }
    }

    /// The next time this task runs after `now`, in seconds since 1970, in the Mac's time zone.
    pub fn next_run(&self, now: u64) -> Option<u64> {
        let t = now as libc::time_t;
        // SAFETY: localtime_r and mktime only read and write the tm we hand them.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return None;
        }
        for add in 0..8 {
            let mut c = tm;
            c.tm_mday += add;
            c.tm_hour = self.hour as i32;
            c.tm_min = self.minute as i32;
            c.tm_sec = 0;
            c.tm_isdst = -1;
            let at = unsafe { libc::mktime(&mut c) };
            if at <= t {
                continue;
            }
            if self.weekday.is_none_or(|d| d as i32 == c.tm_wday) {
                return Some(at as u64);
            }
        }
        None
    }
}

fn dir(env: &Env) -> PathBuf {
    env.home.join("Library/Application Support/mac-anal")
}

fn read<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{} : {e}", d.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn tasks(env: &Env) -> Vec<Task> {
    read(&dir(env).join("tasks.json"))
}

pub fn last_runs(env: &Env) -> BTreeMap<u64, LastRun> {
    read(&dir(env).join("tasks-state.json"))
}

fn save(env: &Env, all: &[Task]) -> Result<(), String> {
    write(&dir(env).join("tasks.json"), &all)
}

/// Checks a task, saves it with a fresh id and loads its agent.
pub fn schedule(env: &Env, launchctl: &str, mut task: Task) -> Result<Task, String> {
    task.check(env)?;
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = tasks(env);
    task.id = all.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    task.paused = false;
    launchd::install(launchctl, env, &task.agent()?)?;
    all.push(task.clone());
    save(env, &all)?;
    Ok(task)
}

/// Pauses (unloads the agent, keeps the task) or resumes (loads it again).
pub fn set_paused(env: &Env, launchctl: &str, id: u64, paused: bool) -> Result<(), String> {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = tasks(env);
    let t = all.iter_mut().find(|t| t.id == id).ok_or("Tâche introuvable.")?;
    if paused {
        launchd::uninstall(launchctl, env, &format!("task-{id}"))?;
    } else {
        t.check(env)?;
        launchd::install(launchctl, env, &t.agent()?)?;
    }
    t.paused = paused;
    save(env, &all)
}

/// Unloads the agent, moves its plist to the Trash and forgets the task.
pub fn remove(env: &Env, launchctl: &str, id: u64) -> Result<(), String> {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    launchd::uninstall(launchctl, env, &format!("task-{id}"))?;
    let mut all = tasks(env);
    all.retain(|t| t.id != id);
    save(env, &all)
}

/// Runs a task now, as launchd does: checks it again, runs it, writes the journal and the result.
pub fn run(env: &Env, id: u64) -> Result<LastRun, String> {
    let task = tasks(env).into_iter().find(|t| t.id == id).ok_or_else(|| format!("Aucune tâche {id}."))?;
    if task.paused {
        return Err("Tâche en pause.".into());
    }
    let result = task.check(env).and_then(|_| match &task.what {
        What::Action(a) => {
            let p = actions::prepare(env, a.clone())?;
            journal::run(env, Origin::Scheduled, &p, false).map(|d| d.summary)
        }
        What::Script(name) => {
            let s = scripts::list(env).into_iter().find(|s| &s.name == name).ok_or("Script introuvable.")?;
            let r = scripts::run(&s.script, false)?;
            journal::record_script(env, Origin::Scheduled, &s.purpose, &s.script, &r);
            if r.status == Some(0) {
                Ok("Script terminé sans erreur.".to_string())
            } else {
                Err(format!("Script terminé avec le code {:?}.", r.status))
            }
        }
    });
    let last = match &result {
        Ok(s) => LastRun { at: journal::now(), ok: true, summary: s.clone() },
        Err(e) => LastRun { at: journal::now(), ok: false, summary: e.clone() },
    };
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut runs = last_runs(env);
    runs.insert(id, last.clone());
    write(&dir(env).join("tasks-state.json"), &runs)?;
    result.map(|_| last)
}

/// `mac-anal run-task <id>`, run by launchd. Returns the exit code.
pub fn run_task_command(id: Option<&str>) -> i32 {
    let (Some(env), Some(id)) = (Env::current(), id.and_then(|i| i.parse().ok())) else { return 2 };
    match run(&env, id) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("mac-anal run-task {id}: {e}");
            1
        }
    }
}

const SYSTEM: &str = "You turn a sentence of a Mac user into one scheduled task for mac-anal: a time \
and one thing to run. weekday is 0 (Sunday) to 6, or -1 for every day; hour 0-23; minute 0-59 \
(default 9:00). Run either a reversible action of the catalog or a script of the library by its \
name. name is a short French title.";

/// Asks Claude for a task from a sentence. Call it from a thread.
pub fn from_sentence(program: &str, env: &Env, sentence: &str) -> Result<Task, String> {
    let library: Vec<String> = scripts::list(env).into_iter().map(|s| format!("- {}: {}", s.name, s.purpose)).collect();
    let action = |k: &str, props: serde_json::Value, req: &[&str]| {
        let mut p = json!({"kind": {"const": k}});
        for (key, v) in props.as_object().unwrap() {
            p[key] = v.clone();
        }
        let mut r = vec!["kind"];
        r.extend_from_slice(req);
        json!({"type": "object", "properties": p, "required": r, "additionalProperties": false})
    };
    let req = ai::Request {
        system: SYSTEM.into(),
        prompt: format!(
            "Catalog (reversible actions):\n- clear_cache {{name}}: a folder of ~/Library/Caches to the Trash\n- disable_login_item {{label}}\n- trash_old_items {{folder: derived_data|caches|downloads, days}}\n\nScripts of the library:\n{}\n\nThe user says: {sentence}",
            if library.is_empty() { "(none)".to_string() } else { library.join("\n") }
        ),
        schema: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "weekday": {"type": "integer"},
                "hour": {"type": "integer"},
                "minute": {"type": "integer"},
                "run": {"anyOf": [
                    action("clear_cache", json!({"name": {"type": "string"}}), &["name"]),
                    action("disable_login_item", json!({"label": {"type": "string"}}), &["label"]),
                    action("trash_old_items", json!({"folder": {"enum": ["derived_data", "caches", "downloads"]}, "days": {"type": "integer"}}), &["folder", "days"]),
                    action("script", json!({"script": {"type": "string"}}), &["script"]),
                ]}
            },
            "required": ["name", "weekday", "hour", "minute", "run"],
            "additionalProperties": false
        }),
    };
    let v = ai::ask(program, &req)?;
    let what = if v["run"]["kind"] == "script" {
        What::Script(v["run"]["script"].as_str().unwrap_or_default().to_string())
    } else {
        What::Action(serde_json::from_value(v["run"].clone()).map_err(|e| format!("Action illisible : {e}"))?)
    };
    let num = |k: &str| v[k].as_i64().unwrap_or(-1);
    let task = Task {
        id: 0,
        name: v["name"].as_str().unwrap_or("Tâche").to_string(),
        weekday: u8::try_from(num("weekday")).ok(),
        hour: u8::try_from(num("hour")).map_err(|_| "Heure invalide.")?,
        minute: u8::try_from(num("minute")).map_err(|_| "Minute invalide.")?,
        what,
        paused: false,
    };
    Ok(task)
}

/// Invented tasks and a proposal for the screenshot mode (`MAC_ANAL_TASKS=1`).
pub fn sample() -> (Vec<Task>, BTreeMap<u64, LastRun>, Task) {
    let t = |id, name: &str, weekday, hour, what| Task { id, name: name.into(), weekday, hour, minute: 0, what, paused: false };
    let mut runs = BTreeMap::new();
    runs.insert(1, LastRun { at: journal::now() - 3 * 86_400, ok: true, summary: "12 éléments sont dans la Corbeille.".into() });
    (
        vec![
            t(1, "Ménage des DerivedData", Some(1), 9, What::Action(Action::TrashOldItems { folder: "derived_data".into(), days: 30 })),
            t(2, "Taille des Téléchargements", None, 18, What::Script("taille-telechargements".into())),
        ],
        runs,
        t(0, "Vieux téléchargements", Some(5), 17, What::Action(Action::TrashOldItems { folder: "downloads".into(), days: 90 })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> (tempfile::TempDir, Env) {
        let d = tempfile::tempdir().unwrap();
        let e = Env { home: d.path().join("home") };
        std::fs::create_dir_all(e.caches().join("com.example")).unwrap();
        (d, e)
    }

    fn task(what: What) -> Task {
        Task { id: 0, name: "t".into(), weekday: Some(1), hour: 9, minute: 0, what, paused: false }
    }

    #[test]
    fn only_reversible_actions_and_read_only_scripts_are_scheduled() {
        let (_d, env) = env();
        assert!(task(What::Action(Action::ClearCache { name: "com.example".into() })).check(&env).is_ok());
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert!(task(What::Action(Action::QuitProcess { pid: child.id() })).check(&env).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        scripts::save(&env, "Lire", "du -sh ~/Library", 1).unwrap();
        scripts::save(&env, "Écrire", "echo x > ~/f", 2).unwrap();
        scripts::save(&env, "Root", "sudo ls", 3).unwrap();
        assert!(task(What::Script("lire".into())).check(&env).is_ok());
        assert!(task(What::Script("ecrire".into())).check(&env).is_err());
        assert!(task(What::Script("root".into())).check(&env).is_err());
        assert!(task(What::Script("absent".into())).check(&env).is_err());
        let mut bad = task(What::Action(Action::ClearCache { name: "com.example".into() }));
        bad.hour = 24;
        assert!(bad.check(&env).is_err());
    }

    #[test]
    fn a_task_is_scheduled_paused_and_removed() {
        let (d, env) = env();
        let fake = crate::launchd::tests::fake_launchctl(d.path());
        let fake = fake.to_str().unwrap();
        let t = schedule(&env, fake, task(What::Action(Action::ClearCache { name: "com.example".into() }))).unwrap();
        assert_eq!(t.id, 1);
        let plist = env.launch_agents().join("ai.c2poly.mac-anal.task-1.plist");
        let lint = std::process::Command::new("plutil").arg("-lint").arg(&plist).output().unwrap();
        assert!(lint.status.success(), "{}", String::from_utf8_lossy(&lint.stdout));
        set_paused(&env, fake, 1, true).unwrap();
        assert!(!plist.exists() && tasks(&env)[0].paused);
        set_paused(&env, fake, 1, false).unwrap();
        assert!(plist.exists());
        remove(&env, fake, 1).unwrap();
        assert!(!plist.exists() && tasks(&env).is_empty());
    }

    #[test]
    fn a_run_does_the_action_and_records_it() {
        let (d, env) = env();
        let fake = crate::launchd::tests::fake_launchctl(d.path());
        let t = schedule(&env, fake.to_str().unwrap(), task(What::Action(Action::ClearCache { name: "com.example".into() }))).unwrap();
        let last = run(&env, t.id).unwrap();
        assert!(last.ok);
        assert!(!env.caches().join("com.example").exists());
        assert_eq!(journal::load(&env)[0].origin, Origin::Scheduled);
        assert_eq!(last_runs(&env)[&t.id], last);
        // The cache is gone now: the next run fails, and says so.
        assert!(run(&env, t.id).is_err());
        assert!(!last_runs(&env)[&t.id].ok);
    }

    #[test]
    fn the_next_run_is_on_the_right_day_and_time() {
        let now = journal::now();
        let t = task(What::Script("x".into()));
        let next = t.next_run(now).unwrap();
        assert!(next > now && next <= now + 7 * 86_400 + 3600);
        let at = next as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&at, &mut tm) };
        assert_eq!((tm.tm_wday, tm.tm_hour, tm.tm_min), (1, 9, 0));
        let daily = Task { weekday: None, ..t };
        assert!(daily.next_run(now).unwrap() <= now + 86_400 + 3600);
        assert_eq!(daily.schedule(), "chaque jour à 09:00");
    }

    #[test]
    fn a_stand_in_claude_gives_a_task() {
        let (d, env) = env();
        let script = d.path().join("claude");
        std::fs::write(&script, "#!/bin/sh\necho '{\"is_error\":false,\"structured_output\":{\"name\":\"Ménage\",\"weekday\":1,\"hour\":9,\"minute\":0,\"run\":{\"kind\":\"trash_old_items\",\"folder\":\"derived_data\",\"days\":30}}}'\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let t = from_sentence(script.to_str().unwrap(), &env, "chaque lundi, vide les DerivedData de plus de 30 jours").unwrap();
        assert_eq!(t.schedule(), "chaque lundi à 09:00");
        assert_eq!(t.what, What::Action(Action::TrashOldItems { folder: "derived_data".into(), days: 30 }));
    }

    /// Asks the real `claude` for the issue's example task, about a cent: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_claude_schedules_the_derived_data_cleanup() {
        let (_d, env) = env();
        let t = from_sentence(&crate::ai::command(), &env, "chaque lundi, vide les DerivedData de plus de 30 jours").unwrap();
        eprintln!("{t:?}");
        assert_eq!(t.weekday, Some(1));
        assert_eq!(t.what, What::Action(Action::TrashOldItems { folder: "derived_data".into(), days: 30 }));
    }
}
