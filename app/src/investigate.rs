//! The assistant's investigation: Claude gets no tools of its own. At each step it answers with a
//! read-only tool from a closed list, which the app checks and runs without a shell, or with its
//! final answer. The app feeds each output back, up to a few steps.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::actions::{Env, CATALOG};
use crate::ai;

/// Steps before Claude must answer.
pub const MAX_STEPS: usize = 6;
/// What a tool may send back to Claude.
const MAX_OUTPUT: usize = 6000;
/// A tool still running after this is stopped.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

const PROFILER_TYPES: &[&str] = &["SPHardwareDataType", "SPPowerDataType", "SPStorageDataType", "SPSoftwareDataType", "SPDisplaysDataType"];

/// A read-only tool, as Claude asks for it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum Tool {
    /// The processes using the most processor.
    Ps {},
    /// The size of the folders under a path of the home folder.
    Du { path: String },
    /// The unified log of the last minutes, optionally filtered.
    LogShow { minutes: u32, predicate: Option<String> },
    /// The agents loaded for the user.
    LaunchctlList {},
    /// One section of the system report.
    SystemProfiler { datatype: String },
    /// The end of the power management log (sleeps, wakes, thermal events).
    PmsetLog {},
    /// The current thermal and performance warnings.
    PmsetTherm {},
}

impl Tool {
    /// The command line, checked: nothing outside the home folder, bounded durations, known types.
    pub fn command(&self, env: &Env) -> Result<(&'static str, Vec<String>), String> {
        Ok(match self {
            Tool::Ps {} => ("ps", vec!["-axo".into(), "pid,user,%cpu,%mem,etime,comm".into(), "-r".into()]),
            Tool::Du { path } => ("du", vec!["-d".into(), "1".into(), "-k".into(), inside_home(env, path)?.display().to_string()]),
            Tool::LogShow { minutes, predicate } => {
                if !(1..=120).contains(minutes) {
                    return Err(format!("Durée hors limites : {minutes} min (1 à 120)."));
                }
                let mut args = vec!["show".into(), "--last".into(), format!("{minutes}m"), "--style".into(), "compact".into()];
                if let Some(p) = predicate.as_deref().filter(|p| !p.trim().is_empty()) {
                    if p.len() > 300 {
                        return Err("Prédicat trop long.".into());
                    }
                    args.extend(["--predicate".into(), p.to_string()]);
                }
                ("log", args)
            }
            Tool::LaunchctlList {} => ("launchctl", vec!["list".into()]),
            Tool::SystemProfiler { datatype } => {
                if !PROFILER_TYPES.contains(&datatype.as_str()) {
                    return Err(format!("Type de rapport inconnu : {datatype}."));
                }
                ("system_profiler", vec![datatype.clone()])
            }
            Tool::PmsetLog {} => ("pmset", vec!["-g".into(), "log".into()]),
            Tool::PmsetTherm {} => ("pmset", vec!["-g".into(), "therm".into()]),
        })
    }

    /// How the step reads in the app.
    pub fn label(&self) -> String {
        match self {
            Tool::Ps {} => "Processus les plus actifs".into(),
            Tool::Du { path } => format!("Taille des dossiers de {path}"),
            Tool::LogShow { minutes, predicate: Some(p) } => format!("Journal système, {minutes} min, filtre « {p} »"),
            Tool::LogShow { minutes, predicate: None } => format!("Journal système, {minutes} min"),
            Tool::LaunchctlList {} => "Agents chargés".into(),
            Tool::SystemProfiler { datatype } => format!("Rapport système {datatype}"),
            Tool::PmsetLog {} => "Journal d'alimentation".into(),
            Tool::PmsetTherm {} => "État thermique".into(),
        }
    }

    /// Long logs matter by their end; reports and lists by their start.
    fn keeps_tail(&self) -> bool {
        matches!(self, Tool::LogShow { .. } | Tool::PmsetLog {})
    }
}

/// A path of the home folder, `~` allowed, resolved: anything that leaves it is refused.
fn inside_home(env: &Env, path: &str) -> Result<PathBuf, String> {
    let raw = match path.strip_prefix('~') {
        Some(rest) => env.home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(path),
    };
    let real = raw.canonicalize().map_err(|_| format!("Chemin introuvable : {path}."))?;
    let home = env.home.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&home) {
        return Err(format!("{path} est hors du dossier personnel."));
    }
    Ok(real)
}

/// Runs a checked tool and returns its output, cut to what Claude may read.
pub fn run(env: &Env, tool: &Tool) -> Result<String, String> {
    let (program, args) = tool.command(env)?;
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program} : {e}"))?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_end(&mut s, &mut buf);
        }
        buf
    });
    let start = Instant::now();
    let mut timed_out = false;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if start.elapsed() > TOOL_TIMEOUT {
            let _ = child.kill();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.wait();
    let text = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    let mut out = cut(&text, MAX_OUTPUT, tool.keeps_tail());
    if timed_out {
        out.push_str("\n[stopped after 30 s]");
    }
    Ok(out)
}

/// At most `max` bytes, on a character boundary, keeping the start or the end.
fn cut(text: &str, max: usize, tail: bool) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    if tail {
        let mut i = text.len() - max;
        while !text.is_char_boundary(i) {
            i += 1;
        }
        format!("[…]\n{}", &text[i..])
    } else {
        let mut i = max;
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        format!("{}\n[…]", &text[..i])
    }
}

/// What the investigation reports as it goes.
pub enum Progress {
    Step { label: String, lines: usize },
    /// The final answer: message, evidence and actions, as JSON for the plan.
    Answer(Value),
    Failed(String),
}

const SYSTEM: &str = "You are the assistant of mac-anal, an app that analyses the user's Mac. \
You investigate before answering: at each step, either ask for one read-only tool, which the app runs \
and shows you, or give your final answer. You never run anything yourself. The final answer has a short \
message in French giving the cause you found, the evidence it rests on (excerpts of tool outputs, \
process names, sizes), and actions taken only from the catalog. Only use names, labels and pids seen in \
the context or in tool outputs; never invent one. Prefer actions that can be undone. When no action fits, \
return an empty list and say why.";

/// Runs the investigation to its answer, reporting each step. `program` is the `claude` command.
pub fn investigate(program: &str, env: &Env, question: &str, context: &str, report: impl Fn(Progress)) {
    let mut transcript = String::new();
    for step in 1..=MAX_STEPS {
        let last = step == MAX_STEPS;
        let req = ai::Request { system: SYSTEM.into(), prompt: prompt(question, context, &transcript, MAX_STEPS - step), schema: schema(last) };
        let reply = match ai::ask(program, &req) {
            Ok(v) => v,
            Err(e) => return report(Progress::Failed(e)),
        };
        let next = reply.get("step").cloned().unwrap_or(Value::Null);
        if let Some(answer) = next.get("answer") {
            return report(Progress::Answer(answer.clone()));
        }
        let output = match serde_json::from_value::<Tool>(next.clone()) {
            Ok(tool) => {
                let label = tool.label();
                let out = run(env, &tool).unwrap_or_else(|e| format!("[refused: {e}]"));
                report(Progress::Step { label: label.clone(), lines: out.lines().count() });
                format!("Step {step}: {label} ({next})\nOutput:\n{out}\n\n")
            }
            Err(e) => format!("Step {step}: invalid tool request {next}: {e}\n\n"),
        };
        transcript.push_str(&output);
    }
    report(Progress::Failed("L'enquête n'a pas abouti dans le nombre d'étapes prévu.".into()))
}

fn prompt(question: &str, context: &str, transcript: &str, steps_left: usize) -> String {
    let catalog: Vec<String> = CATALOG.iter().map(|(k, d)| format!("- {k}: {d}")).collect();
    let tools = "- ps: processes using the most processor\n\
        - du {path}: folder sizes under a path of the home folder (~ allowed)\n\
        - log_show {minutes 1-120, predicate?}: unified log of the last minutes, optionally filtered by an NSPredicate\n\
        - launchctl_list: agents loaded for the user\n\
        - system_profiler {datatype}: one of SPHardwareDataType, SPPowerDataType, SPStorageDataType, SPSoftwareDataType, SPDisplaysDataType\n\
        - pmset_log: end of the power management log (sleep, wake, thermal events)\n\
        - pmset_therm: current thermal and performance warnings";
    let steps = if steps_left == 0 { "This is the last step: give your final answer now.".to_string() } else { format!("Tool steps left after this one: {steps_left}.") };
    format!(
        "Read-only tools:\n{tools}\n\nCatalog of actions:\n{}\n\nWhat mac-anal read on the Mac:\n{context}\n\nThe user asks: {question}\n\nInvestigation so far:\n{}\n{steps}",
        catalog.join("\n"),
        if transcript.is_empty() { "(nothing yet)\n" } else { transcript }
    )
}

/// One step: a tool from the list or the final answer. At the last step, only the answer.
pub fn schema(answer_only: bool) -> Value {
    let tool = |name: &str, fields: Value, required: &[&str]| {
        let mut props = json!({"tool": {"const": name}});
        for (k, v) in fields.as_object().unwrap() {
            props[k] = v.clone();
        }
        let mut req = vec!["tool"];
        req.extend_from_slice(required);
        json!({"type": "object", "properties": props, "required": req, "additionalProperties": false})
    };
    let action = |k: &str, field: &str, ty: &str| {
        json!({"type": "object", "properties": {"kind": {"const": k}, field: {"type": ty}}, "required": ["kind", field], "additionalProperties": false})
    };
    let answer = json!({
        "type": "object",
        "properties": {"answer": {
            "type": "object",
            "properties": {
                "message": {"type": "string"},
                "evidence": {"type": "array", "items": {"type": "string"}},
                "actions": {"type": "array", "items": {"anyOf": [
                    action("clear_cache", "name", "string"),
                    action("quit_process", "pid", "integer"),
                    action("disable_login_item", "label", "string"),
                ]}}
            },
            "required": ["message", "evidence", "actions"],
            "additionalProperties": false
        }},
        "required": ["answer"],
        "additionalProperties": false
    });
    let mut options = vec![answer];
    if !answer_only {
        options.extend([
            tool("ps", json!({}), &[]),
            tool("du", json!({"path": {"type": "string"}}), &["path"]),
            tool("log_show", json!({"minutes": {"type": "integer"}, "predicate": {"type": "string"}}), &["minutes"]),
            tool("launchctl_list", json!({}), &[]),
            tool("system_profiler", json!({"datatype": {"enum": PROFILER_TYPES}}), &["datatype"]),
            tool("pmset_log", json!({}), &[]),
            tool("pmset_therm", json!({}), &[]),
        ]);
    }
    json!({"type": "object", "properties": {"step": {"anyOf": options}}, "required": ["step"], "additionalProperties": false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        (dir, env)
    }

    #[test]
    fn tool_requests_are_checked() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.home.join("Library/Caches")).unwrap();
        assert!(Tool::Du { path: "~/Library".into() }.command(&env).is_ok());
        assert!(Tool::Du { path: "/etc".into() }.command(&env).is_err());
        assert!(Tool::Du { path: "~/../../etc".into() }.command(&env).is_err());
        assert!(Tool::Du { path: "~/missing".into() }.command(&env).is_err());
        assert!(Tool::LogShow { minutes: 0, predicate: None }.command(&env).is_err());
        assert!(Tool::LogShow { minutes: 500, predicate: None }.command(&env).is_err());
        assert!(Tool::SystemProfiler { datatype: "SPNetworkDataType".into() }.command(&env).is_err());
        assert!(serde_json::from_value::<Tool>(json!({"tool": "rm", "path": "/"})).is_err());
        assert!(serde_json::from_value::<Tool>(json!({"tool": "ps", "args": "-x; rm"})).is_err());
        let (prog, args) = Tool::LogShow { minutes: 10, predicate: Some("process == \"kernel\"".into()) }.command(&env).unwrap();
        assert_eq!(prog, "log");
        assert_eq!(args.last().unwrap(), "process == \"kernel\"");
    }

    #[test]
    fn outputs_are_cut_on_character_boundaries() {
        let text = "é".repeat(5000);
        let head = cut(&text, 101, false);
        assert!(head.ends_with("[…]") && head.len() <= 101 + "\n[…]".len());
        let tail = cut(&text, 101, true);
        assert!(tail.starts_with("[…]"));
        assert_eq!(cut("short", 100, true), "short");
    }

    #[test]
    fn real_read_only_tools_run() {
        let (_d, env) = home();
        let ps = run(&env, &Tool::Ps {}).unwrap();
        assert!(ps.contains("PID"), "{ps}");
        assert!(!run(&env, &Tool::LaunchctlList {}).unwrap().is_empty());
        assert!(run(&env, &Tool::PmsetTherm {}).is_ok());
        std::fs::create_dir_all(env.home.join("a")).unwrap();
        assert!(run(&env, &Tool::Du { path: "~".into() }).unwrap().contains("/a"));
    }

    #[test]
    fn the_last_step_only_allows_an_answer() {
        assert_eq!(schema(true)["properties"]["step"]["anyOf"].as_array().unwrap().len(), 1);
        assert_eq!(schema(false)["properties"]["step"]["anyOf"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn a_stand_in_claude_asks_a_tool_then_answers() {
        let (dir, env) = home();
        let count = dir.path().join("count");
        let script = dir.path().join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ -f {c} ]; then echo '{{\"is_error\":false,\"structured_output\":{{\"step\":{{\"answer\":{{\"message\":\"Cause trouvée\",\"evidence\":[\"ps\"],\"actions\":[]}}}}}}}}'; else touch {c}; echo '{{\"is_error\":false,\"structured_output\":{{\"step\":{{\"tool\":\"ps\"}}}}}}'; fi\n",
                c = count.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let seen = Mutex::new(Vec::new());
        investigate(script.to_str().unwrap(), &env, "Pourquoi ?", "ctx", |p| {
            seen.lock().unwrap().push(match p {
                Progress::Step { label, lines } => format!("step {label} {}", lines > 0),
                Progress::Answer(v) => format!("answer {}", v["message"]),
                Progress::Failed(e) => format!("failed {e}"),
            })
        });
        assert_eq!(*seen.lock().unwrap(), vec!["step Processus les plus actifs true".to_string(), "answer \"Cause trouvée\"".into()]);
    }

    #[test]
    fn an_investigation_that_never_answers_stops() {
        let (dir, env) = home();
        let script = dir.path().join("claude");
        std::fs::write(&script, "#!/bin/sh\necho '{\"is_error\":false,\"structured_output\":{\"step\":{\"tool\":\"pmset_therm\"}}}'\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let steps = Mutex::new(0);
        let failed = Mutex::new(false);
        investigate(script.to_str().unwrap(), &env, "q", "c", |p| match p {
            Progress::Step { .. } => *steps.lock().unwrap() += 1,
            Progress::Failed(_) => *failed.lock().unwrap() = true,
            Progress::Answer(_) => {}
        });
        assert_eq!(*steps.lock().unwrap(), MAX_STEPS);
        assert!(*failed.lock().unwrap());
    }

    /// Runs a real investigation with `claude`, a few cents: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_claude_investigates_then_answers() {
        let env = Env::current().unwrap();
        let log = Mutex::new(Vec::new());
        investigate(&crate::ai::command(), &env, "Quel processus utilise le plus de processeur en ce moment ?", "(none)", |p| {
            log.lock().unwrap().push(match p {
                Progress::Step { label, .. } => format!("step: {label}"),
                Progress::Answer(v) => format!("answer: {v}"),
                Progress::Failed(e) => format!("failed: {e}"),
            })
        });
        let log = log.into_inner().unwrap();
        eprintln!("{log:#?}");
        assert!(log.last().unwrap().starts_with("answer"), "{log:?}");
    }
}
