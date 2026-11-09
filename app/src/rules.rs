//! Surveillance: rules the user states in a sentence, turned once by Claude into a declarative rule
//! (measure, threshold, frequency), then checked by `mac-anal check` from launchd with no network
//! and no AI. A rule alerts when its condition starts to hold, and again only after it cleared.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::actions::Env;
use crate::ai;
use crate::launchd::{self, Agent};
use crate::overview::Overview;

/// Alerts kept: the oldest go first.
const KEEP_ALERTS: usize = 50;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static WRITE: Mutex<()> = Mutex::new(());

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// What a rule watches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// Free space of the startup volume, in percent of its size.
    DiskFreePercent,
    /// Free space of the startup volume, in gigabytes.
    DiskFreeGb,
    /// Memory used, in percent.
    MemoryUsedPercent,
    /// Processor load, in percent.
    CpuPercent,
}

impl Measure {
    pub fn label(self) -> &'static str {
        match self {
            Measure::DiskFreePercent => "espace libre du disque (%)",
            Measure::DiskFreeGb => "espace libre du disque (Go)",
            Measure::MemoryUsedPercent => "mémoire utilisée (%)",
            Measure::CpuPercent => "charge du processeur (%)",
        }
    }

    /// The value in a reading, in the measure's unit.
    pub fn value(self, o: &Overview) -> Option<f64> {
        match self {
            Measure::DiskFreePercent => o.disk.filter(|d| d.total > 0).map(|d| (d.total - d.used.min(d.total)) as f64 * 100.0 / d.total as f64),
            Measure::DiskFreeGb => o.disk.map(|d| d.total.saturating_sub(d.used) as f64 / 1e9),
            Measure::MemoryUsedPercent => o.memory.filter(|m| m.total > 0).map(|m| m.used as f64 * 100.0 / m.total as f64),
            Measure::CpuPercent => o.cpu.map(|c| c as f64 * 100.0),
        }
    }

    fn unit(self) -> &'static str {
        if self == Measure::DiskFreeGb { " Go" } else { " %" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Below,
    Above,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u64,
    pub name: String,
    pub measure: Measure,
    pub op: Op,
    pub threshold: f64,
    /// How often to check, in minutes.
    pub every_minutes: u32,
    #[serde(default)]
    pub paused: bool,
}

impl Rule {
    pub fn condition(&self) -> String {
        let op = if self.op == Op::Below { "sous" } else { "au-dessus de" };
        format!("{} {op} {}{}, vérifié toutes les {} min", self.measure.label(), trim(self.threshold), self.measure.unit(), self.every_minutes)
    }

    pub fn holds(&self, value: f64) -> bool {
        match self.op {
            Op::Below => value < self.threshold,
            Op::Above => value > self.threshold,
        }
    }

    /// A rule as Claude proposes it, checked: a known measure, a threshold that makes sense for it,
    /// a frequency between 1 minute and a day.
    pub fn check(mut self) -> Result<Self, String> {
        if self.name.trim().is_empty() {
            self.name = self.measure.label().to_string();
        }
        let max = if self.measure == Measure::DiskFreeGb { 100_000.0 } else { 100.0 };
        if !(0.0..=max).contains(&self.threshold) {
            return Err(format!("Seuil hors limites : {}.", self.threshold));
        }
        if !(1..=1440).contains(&self.every_minutes) {
            return Err(format!("Fréquence hors limites : {} min (1 à 1440).", self.every_minutes));
        }
        Ok(self)
    }
}

fn trim(x: f64) -> String {
    let s = format!("{x:.1}");
    s.trim_end_matches('0').trim_end_matches('.').replace('.', ",")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub rule_id: u64,
    /// Seconds since 1970.
    pub at: u64,
    pub message: String,
}

/// What `check` remembers between runs: when each rule was last checked and whether it held.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub last_checked: std::collections::BTreeMap<u64, u64>,
    pub holding: std::collections::BTreeSet<u64>,
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

pub fn rules(env: &Env) -> Vec<Rule> {
    read(&dir(env).join("rules.json"))
}

pub fn save_rules(env: &Env, rules: &[Rule]) -> Result<(), String> {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    write(&dir(env).join("rules.json"), &rules)
}

/// Adds a checked rule with a fresh id.
pub fn add(env: &Env, mut rule: Rule) -> Result<Rule, String> {
    let mut all = rules(env);
    rule.id = all.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    all.push(rule.clone());
    save_rules(env, &all)?;
    Ok(rule)
}

pub fn alerts(env: &Env) -> Vec<Alert> {
    read(&dir(env).join("alerts.json"))
}

/// Evaluates the rules due at `now` against a reading. Returns the new alerts; the state and the
/// alerts file are updated. A rule alerts once when its condition starts to hold.
pub fn check(env: &Env, reading: &Overview, now: u64) -> Result<Vec<Alert>, String> {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let state_path = dir(env).join("check-state.json");
    let mut state: State = read(&state_path);
    let mut fresh = Vec::new();
    for rule in rules(env).iter().filter(|r| !r.paused) {
        let due = state.last_checked.get(&rule.id).is_none_or(|t| now >= t + rule.every_minutes as u64 * 60);
        if !due {
            continue;
        }
        state.last_checked.insert(rule.id, now);
        let Some(value) = rule.measure.value(reading) else { continue };
        if rule.holds(value) {
            if state.holding.insert(rule.id) {
                let message = format!("{} : {} vaut {}{}.", rule.name, rule.measure.label(), trim(value), rule.measure.unit());
                fresh.push(Alert { rule_id: rule.id, at: now, message });
            }
        } else {
            state.holding.remove(&rule.id);
        }
    }
    write(&state_path, &state)?;
    if !fresh.is_empty() {
        let mut all = alerts(env);
        all.extend(fresh.iter().cloned());
        let excess = all.len().saturating_sub(KEEP_ALERTS);
        all.drain(..excess);
        write(&dir(env).join("alerts.json"), &all)?;
    }
    Ok(fresh)
}

/// `mac-anal check`, run by launchd: one reading, the rules due, a notification per new alert.
/// No network and no AI. Returns the exit code.
pub fn run_check() -> i32 {
    let Some(env) = Env::current() else { return 1 };
    let (Ok(mut sys), Ok(mut disks)) = (sysinfo::System::new(), sysinfo::Disks::new_with_refreshed_list()) else { return 1 };
    // The processor load needs two readings apart.
    crate::metrics::read(&mut sys, &mut disks, true);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    let reading = crate::metrics::read(&mut sys, &mut disks, false);
    match check(&env, &reading, crate::journal::now()) {
        Ok(fresh) => {
            for a in fresh {
                notify(&a.message);
            }
            0
        }
        Err(e) => {
            eprintln!("mac-anal check: {e}");
            1
        }
    }
}

/// The launchd agent that runs `mac-anal check` every minute.
fn checker() -> Result<Agent, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(Agent { name: "check".into(), program: vec![exe.display().to_string(), "check".into()], interval: 60 })
}

/// Installs the checker while a rule is active, and removes it when none is. `launchctl` is
/// `launchd::LAUNCHCTL`, or a stand-in in tests.
pub fn sync_agent(env: &Env, launchctl: &str) -> Result<(), String> {
    let active = rules(env).iter().any(|r| !r.paused);
    let agent = checker()?;
    if active {
        let current = std::fs::read_to_string(agent.path(env)).ok();
        if current.as_deref() != Some(agent.plist().as_str()) {
            launchd::install(launchctl, env, &agent)?;
        }
    } else if launchd::installed(env, &agent.name) {
        launchd::uninstall(launchctl, env, &agent.name)?;
    }
    Ok(())
}

/// A macOS notification, through `osascript`. Its text is passed as an argument, never in the script.
pub fn notify(message: &str) {
    let script = "on run argv\ndisplay notification (item 1 of argv) with title \"mac-anal\" subtitle \"Ouvre l'alerte depuis la barre des menus\"\nend run";
    let _ = std::process::Command::new("osascript").args(["-e", script, message]).status();
}

const SYSTEM: &str = "You turn a sentence of a Mac user into one surveillance rule for mac-anal. \
The measures are: disk_free_percent (free space of the startup disk, percent), disk_free_gb \
(free space, gigabytes), memory_used_percent, cpu_percent. op is below or above. every_minutes \
is how often to check (default 15). name is a short French title.";

/// Asks Claude for a rule from a sentence. Call it from a thread.
pub fn from_sentence(program: &str, sentence: &str) -> Result<Rule, String> {
    let req = ai::Request {
        system: SYSTEM.into(),
        prompt: format!("The user says: {sentence}"),
        schema: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "measure": {"enum": ["disk_free_percent", "disk_free_gb", "memory_used_percent", "cpu_percent"]},
                "op": {"enum": ["below", "above"]},
                "threshold": {"type": "number"},
                "every_minutes": {"type": "integer"}
            },
            "required": ["name", "measure", "op", "threshold", "every_minutes"],
            "additionalProperties": false
        }),
    };
    let mut v = ai::ask(program, &req)?;
    v["id"] = json!(0);
    let rule: Rule = serde_json::from_value(v).map_err(|e| format!("Règle illisible : {e}"))?;
    rule.check()
}

/// Invented rules, alerts and a proposal for the screenshot mode (`MAC_ANAL_WATCH=1`).
pub fn sample() -> (Vec<Rule>, Vec<Alert>, Rule) {
    let now = crate::journal::now();
    let r = |id, name: &str, measure, op, threshold, every_minutes, paused| Rule { id, name: name.into(), measure, op, threshold, every_minutes, paused };
    (
        vec![
            r(1, "Disque presque plein", Measure::DiskFreePercent, Op::Below, 10.0, 15, false),
            r(2, "Processeur à fond", Measure::CpuPercent, Op::Above, 90.0, 5, true),
        ],
        vec![Alert { rule_id: 1, at: now - 600, message: "Disque presque plein : espace libre du disque (%) vaut 8,4 %.".into() }],
        r(0, "Mémoire saturée", Measure::MemoryUsedPercent, Op::Above, 90.0, 15, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview::Usage;

    const GB: u64 = 1_000_000_000;

    fn env() -> (tempfile::TempDir, Env) {
        let d = tempfile::tempdir().unwrap();
        let e = Env { home: d.path().to_path_buf() };
        (d, e)
    }

    fn reading(free_gb: u64) -> Overview {
        Overview { disk: Some(Usage { used: (500 - free_gb) * GB, total: 500 * GB }), memory: Some(Usage { used: 8 * GB, total: 16 * GB }), cpu: Some(0.5) }
    }

    fn rule(threshold: f64, every: u32) -> Rule {
        Rule { id: 0, name: "Disque presque plein".into(), measure: Measure::DiskFreePercent, op: Op::Below, threshold, every_minutes: every, paused: false }
    }

    #[test]
    fn measures_read_a_reading() {
        let o = reading(50);
        assert_eq!(Measure::DiskFreePercent.value(&o), Some(10.0));
        assert_eq!(Measure::DiskFreeGb.value(&o), Some(50.0));
        assert_eq!(Measure::MemoryUsedPercent.value(&o), Some(50.0));
        assert_eq!(Measure::CpuPercent.value(&o), Some(50.0));
        assert_eq!(Measure::CpuPercent.value(&Overview::default()), None);
        assert_eq!(rule(10.0, 15).condition(), "espace libre du disque (%) sous 10 %, vérifié toutes les 15 min");
    }

    #[test]
    fn proposals_are_checked() {
        assert!(rule(10.0, 15).check().is_ok());
        assert!(rule(150.0, 15).check().is_err());
        assert!(rule(-1.0, 15).check().is_err());
        assert!(rule(10.0, 0).check().is_err());
        assert!(rule(10.0, 5000).check().is_err());
        let mut gb = rule(20.0, 15);
        gb.measure = Measure::DiskFreeGb;
        gb.threshold = 300.0;
        assert!(gb.check().is_ok());
    }

    #[test]
    fn a_rule_alerts_once_until_it_clears() {
        let (_d, env) = env();
        let r = add(&env, rule(10.0, 15)).unwrap();
        assert_eq!(r.id, 1);
        assert!(check(&env, &reading(100), 0).unwrap().is_empty());
        // Not due yet: nothing is checked even though the disk filled up.
        assert!(check(&env, &reading(20), 60).unwrap().is_empty());
        let a = check(&env, &reading(20), 15 * 60).unwrap();
        assert_eq!(a.len(), 1);
        assert!(a[0].message.contains("4 %"), "{}", a[0].message);
        assert!(check(&env, &reading(20), 30 * 60).unwrap().is_empty(), "alerted twice");
        assert!(check(&env, &reading(100), 45 * 60).unwrap().is_empty());
        assert_eq!(check(&env, &reading(20), 60 * 60).unwrap().len(), 1);
        assert_eq!(alerts(&env).len(), 2);
        assert!(alerts(&env)[1].message.starts_with("Disque presque plein"));
    }

    #[test]
    fn a_paused_rule_is_skipped() {
        let (_d, env) = env();
        let mut r = rule(10.0, 1);
        r.paused = true;
        add(&env, r).unwrap();
        assert!(check(&env, &reading(1), 0).unwrap().is_empty());
        assert!(alerts(&env).is_empty());
    }

    #[test]
    fn the_checker_follows_the_active_rules() {
        let (d, env) = env();
        let fake = crate::launchd::tests::fake_launchctl(d.path());
        let fake = fake.to_str().unwrap();
        let r = add(&env, rule(10.0, 15)).unwrap();
        sync_agent(&env, fake).unwrap();
        assert!(launchd::installed(&env, "check"));
        sync_agent(&env, fake).unwrap();
        let calls = std::fs::read_to_string(d.path().join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|l| l.starts_with("bootstrap")).count(), 1, "installed twice: {calls}");
        let mut all = rules(&env);
        all.iter_mut().find(|x| x.id == r.id).unwrap().paused = true;
        save_rules(&env, &all).unwrap();
        sync_agent(&env, fake).unwrap();
        assert!(!launchd::installed(&env, "check"));
    }

    #[test]
    fn a_stand_in_claude_gives_a_checked_rule() {
        let (d, _env) = env();
        let script = d.path().join("claude");
        std::fs::write(&script, "#!/bin/sh\necho '{\"is_error\":false,\"structured_output\":{\"name\":\"Disque\",\"measure\":\"disk_free_percent\",\"op\":\"below\",\"threshold\":10,\"every_minutes\":15}}'\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let r = from_sentence(script.to_str().unwrap(), "préviens-moi si le disque passe sous 10 %").unwrap();
        assert_eq!((r.measure, r.op, r.threshold, r.every_minutes), (Measure::DiskFreePercent, Op::Below, 10.0, 15));
    }

    /// Asks the real `claude` for a rule, about a cent: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_claude_turns_a_sentence_into_a_rule() {
        let r = from_sentence(&crate::ai::command(), "préviens-moi si le disque passe sous 10 %").unwrap();
        eprintln!("{r:?}");
        assert_eq!((r.measure, r.op, r.threshold), (Measure::DiskFreePercent, Op::Below, 10.0));
    }
}
