//! Scripts the assistant writes for needs the catalog does not cover. The app reads each one
//! before anything runs (commands, paths written, `sudo`, network), runs it first in a macOS
//! sandbox that forbids writes and the network, and only then for real, after a confirmation.
//! Kept scripts live in `~/Library/Application Support/mac-anal/scripts/`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::actions::{Env, Level};
use crate::ai;

/// A script run (dry or real) still going after this is stopped.
const TIMEOUT: Duration = Duration::from_secs(60);
/// What a run keeps of its output.
const MAX_OUTPUT: usize = 8000;

/// Commands that change the Mac without writing a file themselves (through a system service), or
/// that stop processes: refused in the dry run.
const SERVICE_COMMANDS: &[&str] = &[
    "sudo", "launchctl", "osascript", "kill", "killall", "pkill", "shutdown", "reboot", "halt", "diskutil", "defaults",
    "tmutil", "pmset", "networksetup", "scutil", "open", "softwareupdate", "systemsetup", "csrutil", "spctl", "dscl",
];
/// Commands that delete for good.
const DELETING: &[&str] = &["rm", "rmdir", "srm", "unlink", "shred", "dd"];
/// Commands that reach the network.
const NETWORK: &[&str] = &["curl", "wget", "nc", "ncat", "ssh", "scp", "sftp", "rsync", "ftp", "telnet", "ping", "nslookup", "dig", "git", "brew", "pip", "pip3", "npm"];

/// What the app reads in a script before anything runs. The reading is word by word: it can miss
/// what a script builds at run time, which is why the dry run follows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub commands: Vec<String>,
    pub writes: Vec<String>,
    pub sudo: bool,
    pub deletes: bool,
    pub network: bool,
}

impl Summary {
    /// A script that writes, deletes or calls a service cannot be undone by mac-anal.
    pub fn level(&self) -> Level {
        if self.sudo || self.deletes || !self.writes.is_empty() || self.network || self.commands.iter().any(|c| SERVICE_COMMANDS.contains(&c.as_str())) {
            Level::Irreversible
        } else {
            Level::Read
        }
    }
}

/// Reads a zsh script: the commands it calls, the paths it writes to, `sudo`, deletions, network.
pub fn summarize(script: &str) -> Summary {
    let mut s = Summary::default();
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // A separator glued to a word (`a;`) counts as its own word.
        let words: Vec<&str> = line
            .split_whitespace()
            .flat_map(|w| match w.strip_suffix(';') {
                Some(head) if !head.is_empty() => vec![head, ";"],
                _ => vec![w],
            })
            .collect();
        // Redirections write wherever they point.
        for (i, w) in words.iter().enumerate() {
            for op in [">>", ">"] {
                if let Some(rest) = w.strip_prefix(op) {
                    let target = if rest.is_empty() { words.get(i + 1).copied().unwrap_or("") } else { rest };
                    if !target.is_empty() && target != "/dev/null" && !target.starts_with('&') {
                        push(&mut s.writes, target);
                    }
                    break;
                }
            }
        }
        // Each simple command: the first word after a separator, past assignments and `sudo`.
        let mut expect_command = true;
        let mut current: Option<String> = None;
        let mut args: Vec<&str> = Vec::new();
        let flush = |s: &mut Summary, current: &Option<String>, args: &[&str]| {
            let Some(c) = current else { return };
            if matches!(c.as_str(), "cp" | "mv" | "touch" | "mkdir" | "tee" | "ln" | "chmod" | "chown" | "rm" | "rmdir" | "srm" | "unlink") {
                let paths: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
                let written: Vec<&str> = if matches!(c.as_str(), "cp" | "mv" | "ln") { paths.last().copied().into_iter().collect() } else { paths };
                for p in written {
                    push(&mut s.writes, p);
                }
            }
        };
        for w in &words {
            if matches!(*w, ";" | "|" | "&&" | "||" | "&" | "then" | "do" | "else" | "{" | "}" | "(" | ")") {
                flush(&mut s, &current, &args);
                current = None;
                args.clear();
                expect_command = true;
                continue;
            }
            if w.starts_with('>') || w.starts_with("2>") {
                continue;
            }
            if expect_command {
                let word = w.trim_start_matches(['(', '$', '`']);
                if word.contains('=') && !word.starts_with('=') {
                    continue;
                }
                if matches!(word, "if" | "while" | "for" | "until" | "!" | "time" | "exec" | "command" | "nohup") {
                    continue;
                }
                let name = word.rsplit('/').next().unwrap_or(word).to_string();
                if name == "sudo" {
                    s.sudo = true;
                    push(&mut s.commands, "sudo");
                    continue;
                }
                if DELETING.contains(&name.as_str()) {
                    s.deletes = true;
                }
                if NETWORK.contains(&name.as_str()) {
                    s.network = true;
                }
                push(&mut s.commands, &name);
                current = Some(name);
                expect_command = false;
            } else {
                args.push(w);
            }
        }
        flush(&mut s, &current, &args);
    }
    s
}

fn push(list: &mut Vec<String>, item: &str) {
    let item = item.trim_matches(|c| c == '"' || c == '\'' || c == ';');
    if !item.is_empty() && !list.iter().any(|x| x == item) {
        list.push(item.to_string());
    }
}

/// The dry-run sandbox: reading is allowed; writing files (but /dev/null and the terminal), the
/// network, signals to other processes and the commands that act through system services are not.
fn sandbox_profile() -> String {
    let mut p = String::from(
        "(version 1)\n(allow default)\n(deny file-write*)\n\
         (allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\") (regex #\"^/dev/fd/\"))\n\
         (deny network*)\n(deny signal (target others))\n",
    );
    for c in SERVICE_COMMANDS {
        p.push_str(&format!("(deny process-exec (regex #\"/{c}$\"))\n"));
    }
    p
}

/// How a run went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub status: Option<i32>,
    pub output: String,
    pub timed_out: bool,
}

/// Runs a script with zsh, in the sandbox for a dry run. `program` is the zsh to use (`/bin/zsh`).
pub fn run(script: &str, dry: bool) -> Result<Run, String> {
    let dir = tempfile::Builder::new().prefix("mac-anal-script").tempdir().map_err(|e| e.to_string())?;
    let file = dir.path().join("script.zsh");
    std::fs::write(&file, script).map_err(|e| e.to_string())?;
    let mut cmd = if dry {
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.args(["-p", &sandbox_profile(), "/bin/zsh"]).arg(&file);
        c
    } else {
        let mut c = Command::new("/bin/zsh");
        c.arg(&file);
        c
    };
    let mut child = cmd
        .current_dir(std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("zsh : {e}"))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st.code();
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            timed_out = true;
            break child.wait().ok().and_then(|s| s.code());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut output = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if !stderr.trim().is_empty() {
        output.push_str(&stderr);
    }
    if output.len() > MAX_OUTPUT {
        let mut i = MAX_OUTPUT;
        while !output.is_char_boundary(i) {
            i -= 1;
        }
        output.truncate(i);
        output.push_str("\n[…]");
    }
    Ok(Run { status, output, timed_out })
}

/// Reads a pipe to its end on its own thread, so a chatty script never blocks on a full pipe.
fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut b);
        }
        b
    })
}

/// Bumped when the library changes, so a view knows when to read it again.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// A kept script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub name: String,
    pub purpose: String,
    pub script: String,
    /// Seconds since 1970.
    pub created: u64,
}

pub fn library(env: &Env) -> PathBuf {
    env.home.join("Library/Application Support/mac-anal/scripts")
}

/// A file name from a purpose: lower case letters, digits and dashes.
pub fn slug(purpose: &str) -> String {
    let mut s = String::new();
    for c in purpose.to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s: String = s.trim_end_matches('-').chars().take(48).collect();
    if s.is_empty() { "script".into() } else { s }
}

pub fn save(env: &Env, purpose: &str, script: &str, created: u64) -> Result<Saved, String> {
    let dir = library(env);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    let base = slug(purpose);
    let mut name = base.clone();
    let mut n = 2;
    while dir.join(format!("{name}.json")).exists() {
        name = format!("{base}-{n}");
        n += 1;
    }
    let saved = Saved { name: name.clone(), purpose: purpose.to_string(), script: script.to_string(), created };
    let json = serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{name}.json")), json).map_err(|e| e.to_string())?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(saved)
}

/// The kept scripts, newest first.
pub fn list(env: &Env) -> Vec<Saved> {
    let mut all: Vec<Saved> = std::fs::read_dir(library(env))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice(&b).ok()))
        .collect();
    all.sort_by_key(|s: &Saved| std::cmp::Reverse(s.created));
    all
}

/// Removes a kept script: it goes to the Trash, like everything mac-anal removes.
pub fn delete(env: &Env, name: &str) -> Result<(), String> {
    if name.contains('/') || name.starts_with('.') {
        return Err(format!("« {name} » n'est pas un nom de script."));
    }
    let file = library(env).join(format!("{name}.json"));
    let trash = env.home.join(".Trash");
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    let mut to = trash.join(format!("mac-anal script {name}.json"));
    let mut n = 2;
    while to.exists() {
        to = trash.join(format!("mac-anal script {name} {n}.json"));
        n += 1;
    }
    std::fs::rename(&file, &to).map_err(|e| format!("{} : {e}", file.display()))?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

const SYSTEM: &str = "You write zsh scripts for mac-anal, an app that analyses the user's Mac. \
The app shows the script to the user, runs it first in a read-only sandbox, then for real only after \
the user confirms. Write the shortest script that does what is asked, for macOS, with no sudo unless \
it cannot be avoided, no network unless asked, and never a definitive deletion when moving to \
~/.Trash would do. Comment each step in French. The purpose is one short French sentence.";

/// Asks Claude for a script. Call it from a thread.
pub fn generate(program: &str, need: &str) -> Result<(String, String), String> {
    let req = ai::Request {
        system: SYSTEM.into(),
        prompt: format!("The user needs: {need}"),
        schema: json!({"type": "object", "properties": {"purpose": {"type": "string"}, "script": {"type": "string"}}, "required": ["purpose", "script"], "additionalProperties": false}),
    };
    let v = ai::ask(program, &req)?;
    let purpose = v["purpose"].as_str().unwrap_or_default().to_string();
    let script = v["script"].as_str().ok_or("Claude n'a pas écrit de script.")?.to_string();
    Ok((purpose, script))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_is_read_before_it_runs() {
        let s = summarize("# liste\nls -la ~/Library | head -5\ndu -sh ~/Downloads\n");
        assert_eq!(s.commands, vec!["ls", "head", "du"]);
        assert!(s.writes.is_empty() && !s.sudo && !s.deletes && !s.network);
        assert_eq!(s.level(), Level::Read);

        let s = summarize("X=1 sudo rm -rf /tmp/a; echo ok > ~/out.txt\ncurl -s https://example.com | tee log.txt\nmv a.txt ~/.Trash/\n");
        assert!(s.sudo && s.deletes && s.network);
        assert_eq!(s.commands, vec!["sudo", "rm", "echo", "curl", "tee", "mv"]);
        assert_eq!(s.writes, vec!["~/out.txt", "/tmp/a", "log.txt", "~/.Trash/"]);
        assert_eq!(s.level(), Level::Irreversible);

        assert_eq!(summarize("defaults read com.apple.dock").level(), Level::Irreversible);
        assert_eq!(summarize("echo hi 2>/dev/null").writes, Vec::<String>::new());
    }

    #[test]
    fn the_dry_run_cannot_write_nor_reach_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("written.txt");
        let script = format!("echo dry > {}\nls / >/dev/null && echo read-ok\ncurl -s -m 3 https://example.com >/dev/null || echo net-blocked\n", target.display());
        let r = run(&script, true).unwrap();
        assert!(!target.exists(), "the sandbox let a write through: {}", r.output);
        assert!(r.output.contains("read-ok"), "{}", r.output);
        assert!(r.output.contains("net-blocked"), "{}", r.output);
    }

    #[test]
    fn the_dry_run_refuses_service_commands_and_signals() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let r = run(&format!("/bin/kill {} ; launchctl list >/dev/null && echo launchctl-ran", child.id()), true).unwrap();
        assert!(child.try_wait().unwrap().is_none(), "the sandbox let a signal through: {}", r.output);
        assert!(!r.output.contains("launchctl-ran"), "{}", r.output);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_real_run_writes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("written.txt");
        let r = run(&format!("echo real > {}", target.display()), false).unwrap();
        assert_eq!(r.status, Some(0));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "real\n");
    }

    #[test]
    fn the_library_keeps_lists_and_trashes() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        assert_eq!(slug("Lister les gros fichiers de Téléchargements !"), "lister-les-gros-fichiers-de-telechargements");
        let a = save(&env, "Lister", "ls", 1).unwrap();
        let b = save(&env, "Lister", "ls -la", 2).unwrap();
        assert_eq!((a.name.as_str(), b.name.as_str()), ("lister", "lister-2"));
        assert_eq!(list(&env).iter().map(|s| s.created).collect::<Vec<_>>(), vec![2, 1]);
        delete(&env, "lister").unwrap();
        assert_eq!(list(&env).len(), 1);
        assert!(env.home.join(".Trash/mac-anal script lister.json").is_file());
        assert!(delete(&env, "../x").is_err());
    }

    /// Asks the real `claude` for a script, about a cent: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_claude_writes_a_readable_script() {
        let (purpose, script) = generate(&crate::ai::command(), "Lister les cinq plus gros dossiers de ~/Library/Caches").unwrap();
        eprintln!("{purpose}\n{script}");
        let s = summarize(&script);
        assert!(!purpose.is_empty() && !s.commands.is_empty());
        assert!(!s.sudo && !s.deletes, "{s:?}");
    }
}
