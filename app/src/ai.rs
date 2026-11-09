//! The AI, reached through the `claude` command line: the app keeps no token, Claude Code holds the
//! sign-in. Each call runs with no tools, no MCP server, no settings and no saved session, from a
//! neutral folder, and answers in a JSON schema.

use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use serde_json::Value;

/// One question to the AI.
pub struct Request {
    pub system: String,
    pub prompt: String,
    /// The JSON schema of the answer.
    pub schema: Value,
}

/// The command to run: `claude`, or `MAC_ANAL_CLAUDE` (another path, or a stand-in for tests).
pub fn command() -> String {
    std::env::var("MAC_ANAL_CLAUDE").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "claude".into())
}

/// Asks in a thread and hands back the result by a channel, so the window never waits. `request`
/// is built in that thread too (it may read the disk); `finish` turns the answer into what the
/// caller waits for.
pub fn spawn<T: Send + 'static>(
    program: String,
    request: impl FnOnce() -> Result<Request, String> + Send + 'static,
    finish: impl FnOnce(Result<Value, String>) -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let answer = request().and_then(|req| ask(&program, &req));
        // The receiver is gone when the user gave up waiting: nothing to do then.
        let _ = tx.send(finish(answer));
    });
    rx
}

/// Asks and waits. Call it from a thread: an answer takes seconds.
pub fn ask(program: &str, req: &Request) -> Result<Value, String> {
    let out = Command::new(program)
        .args(["-p", &req.prompt])
        .args(["--output-format", "json", "--json-schema", &req.schema.to_string()])
        .args(["--system-prompt", &req.system])
        .args(["--tools", "", "--strict-mcp-config", "--setting-sources", "", "--no-session-persistence"])
        .args(["--model", "sonnet"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Impossible de lancer « {program} » : {e}. Claude Code est-il installé ?"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() && stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("« {program} » a échoué : {}", stderr.trim()));
    }
    parse_reply(&stdout)
}

/// The structured answer out of `claude -p --output-format json`.
pub fn parse_reply(stdout: &str) -> Result<Value, String> {
    let reply: Value = serde_json::from_str(stdout.trim()).map_err(|e| format!("Réponse illisible de Claude : {e}"))?;
    if reply.get("is_error").and_then(Value::as_bool) == Some(true) {
        let why = reply.get("result").and_then(Value::as_str).unwrap_or("erreur inconnue");
        return Err(format!("Claude a renvoyé une erreur : {why}"));
    }
    if let Some(v) = reply.get("structured_output").filter(|v| !v.is_null()) {
        return Ok(v.clone());
    }
    // Without structured output, the JSON comes as the text of `result`.
    reply
        .get("result")
        .and_then(Value::as_str)
        .and_then(|r| serde_json::from_str(r).ok())
        .ok_or_else(|| "Claude n'a pas répondu dans le format attendu.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> Request {
        Request { system: "s".into(), prompt: "p".into(), schema: json!({"type": "object"}) }
    }

    /// A stand-in for `claude` that prints `reply`.
    fn stand_in(dir: &tempfile::TempDir, reply: &str) -> String {
        let script = dir.path().join("claude");
        std::fs::write(&script, format!("#!/bin/sh\ncat <<'EOF'\n{reply}\nEOF\n")).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script.to_string_lossy().into_owned()
    }

    #[test]
    fn reads_the_structured_output() {
        let v = parse_reply(r#"{"is_error":false,"result":"","structured_output":{"ok":true}}"#).unwrap();
        assert_eq!(v, json!({"ok": true}));
        let v = parse_reply(r#"{"is_error":false,"result":"{\"ok\":false}"}"#).unwrap();
        assert_eq!(v, json!({"ok": false}));
    }

    #[test]
    fn errors_are_reported() {
        assert!(parse_reply(r#"{"is_error":true,"result":"Credit balance is too low"}"#).unwrap_err().contains("Credit"));
        assert!(parse_reply("not json").is_err());
        assert!(parse_reply(r#"{"is_error":false,"result":"plain text"}"#).is_err());
    }

    #[test]
    fn a_stand_in_command_answers_through_the_channel() {
        let dir = tempfile::tempdir().unwrap();
        let program = stand_in(&dir, r#"{"is_error":false,"structured_output":{"message":"ok","actions":[]}}"#);
        let rx = spawn(program, || Ok(request()), |answer| answer.map(|v| v["message"].clone()));
        let got = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(got.unwrap(), "ok");
    }

    #[test]
    fn a_request_that_cannot_be_built_never_calls_claude() {
        let rx = spawn("/nonexistent/claude".into(), || Err("no context".into()), |answer| answer);
        assert_eq!(rx.recv().unwrap().unwrap_err(), "no context");
    }

    #[test]
    fn a_missing_command_says_so() {
        assert!(ask("/nonexistent/claude", &request()).unwrap_err().contains("installé"));
    }

    /// Calls the real `claude`, which costs about a cent: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_claude_answers_in_the_schema() {
        let req = Request {
            system: "You answer only with the requested JSON.".into(),
            prompt: "Set ok to true.".into(),
            schema: json!({"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"]}),
        };
        assert_eq!(ask(&command(), &req).unwrap()["ok"], true);
    }
}
