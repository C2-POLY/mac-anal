//! The LaunchAgents mac-anal installs for the user: a plist in `~/Library/LaunchAgents` with the
//! `ai.c2poly.mac-anal.` prefix, loaded with `launchctl bootstrap` and removed with `bootout`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::actions::Env;

/// The prefix of every agent mac-anal creates, so it only ever touches its own.
pub const PREFIX: &str = "ai.c2poly.mac-anal.";

/// The real command; tests hand a stand-in to the functions instead.
pub const LAUNCHCTL: &str = "launchctl";

/// When launchd starts an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Every so many seconds, and once when loaded.
    Every(u32),
    /// At a time of day, every day or on one weekday (0 is Sunday).
    Calendar { weekday: Option<u8>, hour: u8, minute: u8 },
}

/// An agent that runs a command on a trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    /// The part after the prefix, for example `check`.
    pub name: String,
    pub program: Vec<String>,
    pub trigger: Trigger,
}

impl Agent {
    pub fn label(&self) -> String {
        format!("{PREFIX}{}", self.name)
    }

    pub fn path(&self, env: &Env) -> PathBuf {
        env.launch_agents().join(format!("{}.plist", self.label()))
    }

    pub fn plist(&self) -> String {
        let args: String = self.program.iter().map(|a| format!("\t\t<string>{}</string>\n", xml(a))).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>{}</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n\
             {}\
             \t<key>ProcessType</key>\n\t<string>Background</string>\n\
             </dict>\n</plist>\n",
            xml(&self.label()),
            match self.trigger {
                Trigger::Every(secs) => format!("\t<key>StartInterval</key>\n\t<integer>{secs}</integer>\n\t<key>RunAtLoad</key>\n\t<true/>\n"),
                Trigger::Calendar { weekday, hour, minute } => {
                    let day = weekday.map(|d| format!("\t\t<key>Weekday</key>\n\t\t<integer>{d}</integer>\n")).unwrap_or_default();
                    format!("\t<key>StartCalendarInterval</key>\n\t<dict>\n{day}\t\t<key>Hour</key>\n\t\t<integer>{hour}</integer>\n\t\t<key>Minute</key>\n\t\t<integer>{minute}</integer>\n\t</dict>\n")
                }
            }
        )
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn domain() -> String {
    // SAFETY: getuid cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Writes the plist and loads it with `launchctl` (`LAUNCHCTL`). An agent already loaded is replaced.
pub fn install(launchctl: &str, env: &Env, agent: &Agent) -> Result<(), String> {
    let path = agent.path(env);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    }
    let _ = run(launchctl, &["bootout", &format!("{}/{}", domain(), agent.label())]);
    std::fs::write(&path, agent.plist()).map_err(|e| format!("{} : {e}", path.display()))?;
    run(launchctl, &["bootstrap", &domain(), &path.display().to_string()])
}

/// Unloads the agent and moves its plist to the Trash.
pub fn uninstall(launchctl: &str, env: &Env, name: &str) -> Result<(), String> {
    let agent = Agent { name: name.into(), program: Vec::new(), trigger: Trigger::Every(0) };
    let _ = run(launchctl, &["bootout", &format!("{}/{}", domain(), agent.label())]);
    let path = agent.path(env);
    if path.exists() {
        let trash = env.home.join(".Trash");
        std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
        std::fs::rename(&path, free(&trash, &format!("{}.plist", agent.label()))).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn installed(env: &Env, name: &str) -> bool {
    Agent { name: name.into(), program: Vec::new(), trigger: Trigger::Every(0) }.path(env).is_file()
}

fn free(dir: &Path, name: &str) -> PathBuf {
    let mut p = dir.join(name);
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{n} {name}"));
        n += 1;
    }
    p
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program} : {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} : {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A stand-in `launchctl` that writes its arguments to `calls` in the temp folder.
    pub fn fake_launchctl(dir: &Path) -> PathBuf {
        let script = dir.join("launchctl");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> {}\n", dir.join("calls").display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script
    }

    #[test]
    fn an_agent_is_written_loaded_and_removed() {
        let d = tempfile::tempdir().unwrap();
        let env = Env { home: d.path().join("home") };
        let fake = fake_launchctl(d.path());
        let fake = fake.to_str().unwrap();
        let agent = Agent { name: "check".into(), program: vec!["/Apps/mac anal".into(), "check".into()], trigger: Trigger::Every(60) };
        install(fake, &env, &agent).unwrap();
        let plist = std::fs::read_to_string(agent.path(&env)).unwrap();
        assert!(plist.contains("<string>ai.c2poly.mac-anal.check</string>"));
        assert!(plist.contains("<string>/Apps/mac anal</string>") && plist.contains("<integer>60</integer>"));
        assert!(installed(&env, "check"));
        uninstall(fake, &env, "check").unwrap();
        assert!(!installed(&env, "check"));
        assert!(env.home.join(".Trash/ai.c2poly.mac-anal.check.plist").is_file());
        let calls = std::fs::read_to_string(d.path().join("calls")).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert!(lines[0].starts_with("bootout gui/") && lines[1].starts_with("bootstrap gui/") && lines[2].starts_with("bootout gui/"), "{calls}");
    }

    #[test]
    fn labels_are_escaped() {
        let a = Agent { name: "x".into(), program: vec!["a<b>&\"c\"".into()], trigger: Trigger::Every(5) };
        assert!(a.plist().contains("a&lt;b&gt;&amp;&quot;c&quot;"));
    }

    #[test]
    fn a_calendar_agent_runs_at_its_time() {
        let a = Agent { name: "task-1".into(), program: vec!["x".into()], trigger: Trigger::Calendar { weekday: Some(1), hour: 9, minute: 5 } };
        let p = a.plist();
        assert!(p.contains("<key>StartCalendarInterval</key>") && p.contains("<key>Weekday</key>\n\t\t<integer>1</integer>"));
        assert!(p.contains("<key>Hour</key>\n\t\t<integer>9</integer>") && !p.contains("RunAtLoad"));
        let daily = Agent { trigger: Trigger::Calendar { weekday: None, hour: 9, minute: 5 }, ..a };
        assert!(!daily.plist().contains("Weekday"));
    }
}
