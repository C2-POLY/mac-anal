//! `mac-anal mcp`: an MCP server on stdio (newline-delimited JSON-RPC 2.0), for Claude Desktop and
//! Claude Code. It reads the analysis, the measures and the journal, and runs only the reversible
//! actions of the catalog, journaled with the MCP origin. Irreversible actions are refused.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::actions::{self, Action, Env, Level};
use crate::journal::{self, Origin};

const PROTOCOL: &str = "2025-06-18";

/// Serves requests from stdin until it closes.
pub fn serve() -> i32 {
    let Some(env) = Env::current() else {
        eprintln!("mac-anal mcp: HOME is not set");
        return 1;
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&env, &req),
            Err(e) => Some(error(Value::Null, -32700, &format!("Parse error: {e}"))),
        };
        if let Some(r) = reply {
            if writeln!(stdout, "{r}").and_then(|_| stdout.flush()).is_err() {
                break;
            }
        }
    }
    0
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// One request, one reply; notifications get none.
pub fn handle(env: &Env, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or_default();
    let id = id?; // A notification (no id) never gets a reply.
    let result = match method {
        "initialize" => {
            let version = req["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mac-anal", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "mac-anal analyses this Mac. Read with overview, analysis and journal. prepare_action shows what an action would touch; run_action runs only reversible actions, and each one can be undone with undo."
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or_default();
            let args = req["params"].get("arguments").cloned().unwrap_or(json!({}));
            match call(env, name, &args) {
                Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            }
        }
        _ => return Some(error(id, -32601, &format!("Method not found: {method}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn tools() -> Value {
    let action = json!({"type": "object", "properties": {"action": actions::json_schema()}, "required": ["action"], "additionalProperties": false});
    json!([
        {"name": "overview", "description": "Free disk space, memory and processor load of this Mac, measured now.", "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}},
        {"name": "analysis", "description": "What mac-anal reads to advise: the biggest caches, the login items, the processes using the most memory, and the last journal entries.", "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}},
        {"name": "journal", "description": "The last actions mac-anal ran, with their origin, their measured effect and their id for undo.", "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "minimum": 1, "maximum": 100}}, "additionalProperties": false}},
        {"name": "prepare_action", "description": "Checks an action of the catalog and says what it would touch and its level, without running it.", "inputSchema": action.clone()},
        {"name": "run_action", "description": "Runs a reversible action of the catalog and journals it. Irreversible actions are refused: the user runs those from the app.", "inputSchema": action},
        {"name": "undo", "description": "Puts back what a journal entry did.", "inputSchema": {"type": "object", "properties": {"entry_id": {"type": "integer"}}, "required": ["entry_id"], "additionalProperties": false}},
    ])
}

fn parse_action(args: &Value) -> Result<Action, String> {
    serde_json::from_value(args.get("action").cloned().unwrap_or(Value::Null)).map_err(|e| format!("Unknown or invalid action: {e}"))
}

fn call(env: &Env, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "overview" => {
            let (Ok(mut sys), Ok(mut disks)) = (sysinfo::System::new(), sysinfo::Disks::new_with_refreshed_list()) else {
                return Err("Measures unavailable.".into());
            };
            crate::metrics::read(&mut sys, &mut disks, true);
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            let o = crate::metrics::read(&mut sys, &mut disks, false);
            // The three historical measures only, each on its summary line, as before the dashboard.
            let s = crate::dashboard::Snapshot { overview: o, ..Default::default() };
            use crate::settings::Widget;
            Ok([Widget::Disk, Widget::Memory, Widget::Cpu].iter().map(|w| crate::menubar::detail(*w, &s).swap_remove(0)).collect::<Vec<_>>().join("\n"))
        }
        "analysis" => Ok(crate::assistant::context(env)),
        "journal" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20).clamp(1, 100) as usize;
            let entries = journal::load(env);
            if entries.is_empty() {
                return Ok("The journal is empty.".into());
            }
            Ok(entries
                .iter()
                .rev()
                .take(limit)
                .map(|e| format!("#{} {} [{}] {}: {}{}", e.id, journal::date(e.at), e.origin.label(), e.title, e.effect(), if e.undo.is_some() && !e.undone { " (undo possible)" } else { "" }))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "prepare_action" => {
            let p = actions::prepare(env, parse_action(args)?)?;
            Ok(format!("{} [{}]\n{}", p.title, p.level.label(), p.preview))
        }
        "run_action" => {
            let p = actions::prepare(env, parse_action(args)?)?;
            if p.level != Level::Reversible {
                return Err(format!("Refused: « {} » is {} and cannot be undone. The user can run it from the mac-anal window.", p.title, p.level.label()));
            }
            let done = journal::run(env, Origin::Mcp, &p, false)?;
            Ok(format!("{}\nJournal entry #{} (undo possible).", done.summary, done.entry.unwrap_or_default()))
        }
        "undo" => {
            let id = args.get("entry_id").and_then(Value::as_u64).ok_or("entry_id is required.")?;
            journal::undo(env, id)?;
            Ok(format!("Entry #{id} undone."))
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> (tempfile::TempDir, Env) {
        let d = tempfile::tempdir().unwrap();
        let e = Env { home: d.path().to_path_buf() };
        std::fs::create_dir_all(e.caches().join("com.example")).unwrap();
        (d, e)
    }

    fn call_tool(env: &Env, name: &str, args: Value) -> Value {
        handle(env, &json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap()["result"].clone()
    }

    #[test]
    fn the_handshake_and_the_tools() {
        let (_d, env) = env();
        let init = handle(&env, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "mac-anal");
        assert!(handle(&env, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let list = handle(&env, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["overview", "analysis", "journal", "prepare_action", "run_action", "undo"]);
        assert_eq!(handle(&env, &json!({"jsonrpc": "2.0", "id": 3, "method": "nope"})).unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn a_reversible_action_runs_is_journaled_and_undone() {
        let (_d, env) = env();
        let r = call_tool(&env, "run_action", json!({"action": {"kind": "clear_cache", "name": "com.example"}}));
        assert!(r.get("isError").is_none(), "{r}");
        assert!(!env.caches().join("com.example").exists());
        let e = &journal::load(&env)[0];
        assert_eq!(e.origin, Origin::Mcp);
        assert!(call_tool(&env, "journal", json!({}))["content"][0]["text"].as_str().unwrap().contains("[MCP]"));
        let u = call_tool(&env, "undo", json!({"entry_id": e.id}));
        assert!(u.get("isError").is_none(), "{u}");
        assert!(env.caches().join("com.example").is_dir());
    }

    #[test]
    fn irreversible_and_unknown_actions_are_refused() {
        let (_d, env) = env();
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let r = call_tool(&env, "run_action", json!({"action": {"kind": "quit_process", "pid": child.id()}}));
        assert_eq!(r["isError"], true);
        assert!(child.try_wait().unwrap().is_none(), "the process was signalled");
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(call_tool(&env, "run_action", json!({"action": {"kind": "rm_rf", "path": "/"}}))["isError"], true);
        assert_eq!(call_tool(&env, "shell", json!({}))["isError"], true);
        let p = call_tool(&env, "prepare_action", json!({"action": {"kind": "clear_cache", "name": "com.example"}}));
        assert!(p["content"][0]["text"].as_str().unwrap().contains("Réversible"));
        assert!(env.caches().join("com.example").exists(), "prepare ran the action");
    }

    #[test]
    fn reading_tools_answer() {
        let (_d, env) = env();
        assert!(call_tool(&env, "overview", json!({}))["content"][0]["text"].as_str().unwrap().contains("Disque"));
        assert!(call_tool(&env, "analysis", json!({}))["content"][0]["text"].as_str().unwrap().contains("com.example"));
        assert_eq!(call_tool(&env, "journal", json!({}))["content"][0]["text"], "The journal is empty.");
    }
}
