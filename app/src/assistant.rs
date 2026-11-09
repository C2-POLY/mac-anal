//! The Assistant panel: a question, the AI's plan of actions from the catalog, and for each one
//! what it touches, its level, and Run / Confirm / Undo. The app runs the actions, never the AI.

use std::process::Command;
use std::sync::mpsc::{self, Receiver};

use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use serde_json::{json, Value};

use crate::actions::{self, Action, Done, Env, Level, Prepared, CATALOG};
use crate::ai;

/// What the AI proposed, checked by the app.
pub struct Plan {
    pub message: String,
    pub items: Vec<Item>,
}

/// One proposed action: validated (or refused, with why) and where it stands.
pub struct Item {
    pub proposal: Result<Prepared, String>,
    pub state: State,
}

pub enum State {
    Ready,
    /// An irreversible action waiting for its second click.
    Confirming,
    Done(Done),
    Undone,
    Failed(String),
}

pub struct Assistant {
    env: Option<Env>,
    question: String,
    pending: Option<Receiver<Result<Plan, String>>>,
    plan: Option<Plan>,
    error: Option<String>,
}

impl Assistant {
    pub fn new(sample: bool) -> Self {
        let mut a = Assistant { env: Env::current(), question: String::new(), pending: None, plan: None, error: None };
        if sample {
            a.question = "Libère de la place sur mon disque".into();
            a.plan = Some(sample_plan());
        }
        a
    }

    fn ask(&mut self, ctx: &egui::Context) {
        let Some(env) = self.env.clone() else {
            self.error = Some("Dossier personnel introuvable (HOME).".into());
            return;
        };
        let question = self.question.trim().to_string();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = ai::ask(&ai::command(), &request(&question, &context(&env))).and_then(|v| plan_from(&env, &v));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.pending = Some(rx);
        self.plan = None;
        self.error = None;
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(plan)) => {
                self.plan = Some(plan);
                self.pending = None;
            }
            Ok(Err(e)) => {
                self.error = Some(e);
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = Some("L'analyse s'est arrêtée sans réponse.".into());
                self.pending = None;
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        ui.add_space(8.0);
        ui.heading("Assistant");
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.question)
                .hint_text("Ex. : libère de la place sur mon disque")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        let busy = self.pending.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy && !self.question.trim().is_empty(), egui::Button::new("Demander")).clicked() {
                self.ask(ui.ctx());
            }
            if busy {
                ui.spinner();
                ui.label(RichText::new("Analyse en cours…").weak());
            }
        });
        if let Some(e) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let env = self.env.clone();
        let Some(plan) = &mut self.plan else { return };
        ui.add_space(12.0);
        ui.label(&plan.message);
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, item) in plan.items.iter_mut().enumerate() {
                ui.push_id(i, |ui| item_card(ui, env.as_ref(), item));
                ui.add_space(8.0);
            }
        });
    }
}

fn level_color(ui: &egui::Ui, level: Level) -> Color32 {
    match level {
        Level::Read => ui.visuals().weak_text_color(),
        Level::Reversible => Color32::from_rgb(52, 168, 83),
        Level::Irreversible => ui.visuals().error_fg_color,
    }
}

fn item_card(ui: &mut egui::Ui, env: Option<&Env>, item: &mut Item) {
    let visuals = ui.visuals().clone();
    Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let p = match &item.proposal {
                Ok(p) => p,
                Err(why) => {
                    ui.label(RichText::new("Action refusée").strong());
                    ui.label(RichText::new(why).weak());
                    return;
                }
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.level.label()).small().color(level_color(ui, p.level)));
                ui.label(RichText::new(&p.title).strong());
            });
            ui.label(RichText::new(&p.preview).weak());
            ui.add_space(4.0);
            let mut next = None;
            match &item.state {
                State::Ready => {
                    let text = if p.level == Level::Irreversible { "Lancer…" } else { "Lancer" };
                    if ui.add_enabled(env.is_some(), egui::Button::new(text)).clicked() {
                        next = Some(if p.level == Level::Irreversible { State::Confirming } else { run(env, p, false) });
                    }
                }
                State::Confirming => {
                    ui.colored_label(visuals.error_fg_color, "Cette action ne peut pas être annulée.");
                    ui.horizontal(|ui| {
                        if ui.button("Confirmer").clicked() {
                            next = Some(run(env, p, true));
                        }
                        if ui.button("Renoncer").clicked() {
                            next = Some(State::Ready);
                        }
                    });
                }
                State::Done(done) => {
                    ui.label(&done.summary);
                    if let Some(u) = &done.undo {
                        if ui.button("Annuler").clicked() {
                            next = Some(match actions::undo(u) {
                                Ok(()) => State::Undone,
                                Err(e) => State::Failed(e),
                            });
                        }
                    }
                }
                State::Undone => {
                    ui.label(RichText::new("Annulé : tout est remis en place.").weak());
                }
                State::Failed(e) => {
                    ui.colored_label(visuals.error_fg_color, e);
                }
            }
            if let Some(s) = next {
                item.state = s;
            }
        });
}

fn run(env: Option<&Env>, p: &Prepared, confirmed: bool) -> State {
    let Some(env) = env else { return State::Failed("Dossier personnel introuvable (HOME).".into()) };
    match actions::execute(env, p, confirmed) {
        Ok(done) => State::Done(done),
        Err(e) => State::Failed(e),
    }
}

/// The AI's answer turned into a plan: an unknown action, or one that fails its checks, is kept
/// as refused, with why, and can never run.
pub fn plan_from(env: &Env, v: &Value) -> Result<Plan, String> {
    let message = v.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
    let list = v.get("actions").and_then(Value::as_array).ok_or("La réponse ne contient pas de liste d'actions.")?;
    let items = list
        .iter()
        .map(|raw| {
            let proposal = serde_json::from_value::<Action>(raw.clone())
                .map_err(|e| format!("{raw} : {e}"))
                .and_then(|a| actions::prepare(env, a));
            Item { proposal, state: State::Ready }
        })
        .collect();
    Ok(Plan { message, items })
}

const SYSTEM: &str = "You are the assistant of mac-anal, an app that analyses the user's Mac. \
You never run anything yourself: you answer with a short message in French and a list of actions \
taken only from the catalog below. The app shows each action to the user, who decides to run it. \
Only use names, labels and pids that appear in the context; never invent one. Prefer actions that \
can be undone. When no action fits the question, return an empty list and explain why in the message.";

/// The question with the catalog and what was read on the Mac.
pub fn request(question: &str, context: &str) -> ai::Request {
    let catalog: Vec<String> = CATALOG.iter().map(|(k, d)| format!("- {k}: {d}")).collect();
    let prompt = format!("Catalog of actions:\n{}\n\nWhat mac-anal read on the Mac:\n{context}\n\nThe user asks: {question}", catalog.join("\n"));
    ai::Request { system: SYSTEM.into(), prompt, schema: schema() }
}

/// One object per kind, with only its own field, so nothing else can slip in.
pub fn schema() -> Value {
    let kind = |k: &str, field: &str, ty: &str| {
        json!({"type": "object", "properties": {"kind": {"const": k}, field: {"type": ty}}, "required": ["kind", field], "additionalProperties": false})
    };
    json!({
        "type": "object",
        "properties": {
            "message": {"type": "string"},
            "actions": {"type": "array", "items": {"anyOf": [
                kind("clear_cache", "name", "string"),
                kind("quit_process", "pid", "integer"),
                kind("disable_login_item", "label", "string"),
            ]}}
        },
        "required": ["message", "actions"],
        "additionalProperties": false
    })
}

/// What the AI gets to see, read only: the biggest caches, the user's login items, the processes
/// using the most memory.
fn context(env: &Env) -> String {
    let mut out = String::new();
    let mut caches: Vec<(String, u64)> = std::fs::read_dir(env.caches())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), actions::folder_size(&e.path()).0))
        .collect();
    caches.sort_by_key(|c| std::cmp::Reverse(c.1));
    out.push_str("Biggest folders in ~/Library/Caches:\n");
    for (name, bytes) in caches.iter().take(15) {
        out.push_str(&format!("- {name}: {}\n", crate::overview::format_bytes(*bytes)));
    }
    out.push_str("User login items (~/Library/LaunchAgents):\n");
    for e in std::fs::read_dir(env.launch_agents()).into_iter().flatten().flatten() {
        if let Some(label) = e.file_name().to_string_lossy().strip_suffix(".plist") {
            out.push_str(&format!("- {label}\n"));
        }
    }
    out.push_str("Processes of the user using the most memory (pid, memory, name):\n");
    for line in top_processes(10) {
        out.push_str(&format!("- {line}\n"));
    }
    out
}

fn top_processes(n: usize) -> Vec<String> {
    let user = std::env::var("USER").unwrap_or_default();
    let Ok(out) = Command::new("ps").args(["-axo", "pid=,user=,rss=,comm="]).output() else { return vec![] };
    let mut rows: Vec<(u64, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, u, rss) = (f.next()?, f.next()?, f.next()?.parse::<u64>().ok()?);
            let comm = f.collect::<Vec<_>>().join(" ");
            let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
            (u == user).then(|| (rss, format!("{pid}, {}, {name}", crate::overview::format_bytes(rss * 1024))))
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    rows.into_iter().take(n).map(|r| r.1).collect()
}

/// An invented plan for the screenshot mode (`MAC_ANAL_ASSISTANT=1`): one action in each state.
fn sample_plan() -> Plan {
    let p = |action, level, title: &str, preview: &str| Prepared { action, level, title: title.into(), preview: preview.into() };
    Plan {
        message: "Trois pistes pour libérer de la place : un gros cache, un élément de démarrage inutile et un processus gourmand.".into(),
        items: vec![
            Item {
                proposal: Ok(p(
                    Action::ClearCache { name: "com.example.music".into() },
                    Level::Reversible,
                    "Vider le cache com.example.music",
                    "Envoie ~/Library/Caches/com.example.music à la Corbeille (1204 fichiers, 2,3 Go).",
                )),
                state: State::Ready,
            },
            Item {
                proposal: Ok(p(
                    Action::DisableLoginItem { label: "com.example.updater".into() },
                    Level::Reversible,
                    "Désactiver com.example.updater au démarrage",
                    "Met de côté ~/Library/LaunchAgents/com.example.updater.plist ; il ne se lancera plus à la prochaine ouverture de session.",
                )),
                state: State::Done(Done { summary: "com.example.updater ne se lancera plus au démarrage.".into(), undo: None }),
            },
            Item {
                proposal: Ok(p(
                    Action::QuitProcess { pid: 4242 },
                    Level::Irreversible,
                    "Quitter Example Helper",
                    "Demande au processus 4242 (Example Helper) de quitter. Le travail non enregistré peut être perdu.",
                )),
                state: State::Confirming,
            },
            Item { proposal: Err("Le cache « com.example.gone » n'existe pas.".into()), state: State::Ready },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_keeps_refused_actions_apart() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        std::fs::create_dir_all(env.caches().join("com.example.app")).unwrap();
        let v = json!({"message": "m", "actions": [
            {"kind": "clear_cache", "name": "com.example.app"},
            {"kind": "clear_cache", "name": "../../Documents"},
            {"kind": "format_disk"},
        ]});
        let plan = plan_from(&env, &v).unwrap();
        assert_eq!(plan.message, "m");
        assert!(plan.items[0].proposal.is_ok());
        assert!(plan.items[1].proposal.is_err());
        assert!(plan.items[2].proposal.is_err());
        assert!(plan_from(&env, &json!({"message": "m"})).is_err());
    }

    #[test]
    fn the_schema_and_the_catalog_name_the_same_kinds() {
        let s = schema();
        let kinds: Vec<&str> = s["properties"]["actions"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["properties"]["kind"]["const"].as_str().unwrap())
            .collect();
        let catalog: Vec<&str> = CATALOG.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, catalog);
        assert!(request("q", "ctx").prompt.contains("clear_cache"));
    }

    #[test]
    fn the_whole_loop_runs_with_a_stand_in_claude() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        std::fs::create_dir_all(env.caches().join("com.example.app")).unwrap();
        let script = dir.path().join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\necho '{\"is_error\":false,\"structured_output\":{\"message\":\"Un cache\",\"actions\":[{\"kind\":\"clear_cache\",\"name\":\"com.example.app\"}]}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let v = ai::ask(script.to_str().unwrap(), &request("place", &context(&env))).unwrap();
        let plan = plan_from(&env, &v).unwrap();
        let p = plan.items[0].proposal.as_ref().unwrap();
        let done = actions::execute(&env, p, false).unwrap();
        assert!(!env.caches().join("com.example.app").exists());
        actions::undo(done.undo.as_ref().unwrap()).unwrap();
        assert!(env.caches().join("com.example.app").is_dir());
    }
}
