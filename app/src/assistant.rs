//! The Assistant panel: a question goes to the AI with a read-only picture of the Mac, the answer
//! comes back as a plan of actions from the registry, each validated and previewed before it runs.

use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use serde_json::{json, Value};

use crate::actions::{self, Action, Env, Level, Prepared};
use crate::ai;
use crate::overview::format_bytes;

/// At most this many caches and processes are sent to the AI, the biggest first.
const CONTEXT_TOP: usize = 12;
/// At most this many actions are kept from one answer.
const MAX_ACTIONS: usize = 10;

/// What the AI sees of the Mac. Read only; built in the AI's thread.
#[derive(Default)]
pub struct Context {
    /// Folder name under `~/Library/Caches`, and its size in bytes.
    pub caches: Vec<(String, u64)>,
    /// Labels of `~/Library/LaunchAgents/*.plist`.
    pub login_items: Vec<String>,
    /// The user's processes: pid, name, resident memory in bytes, CPU percent.
    pub processes: Vec<(u32, String, u64, f32)>,
}

impl Context {
    pub fn read(env: &Env) -> Self {
        let mut caches: Vec<(String, u64)> = std::fs::read_dir(env.caches())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), actions::folder_size(&e.path()).0))
            .collect();
        caches.sort_by_key(|c| std::cmp::Reverse(c.1));
        caches.truncate(CONTEXT_TOP);

        let mut login_items: Vec<String> = std::fs::read_dir(env.launch_agents())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".plist").map(str::to_string))
            .collect();
        login_items.sort();

        Self { caches, login_items, processes: user_processes() }
    }

    fn to_json(&self) -> Value {
        json!({
            "caches": self.caches.iter().map(|(n, b)| json!({"name": n, "size": format_bytes(*b)})).collect::<Vec<_>>(),
            "login_items": self.login_items,
            "processes": self.processes.iter().map(|(pid, name, rss, cpu)| {
                json!({"pid": pid, "name": name, "memory": format_bytes(*rss), "cpu_percent": cpu})
            }).collect::<Vec<_>>(),
        })
    }
}

/// The user's processes using the most memory, as `ps` lists them.
fn user_processes() -> Vec<(u32, String, u64, f32)> {
    // SAFETY: getuid has no precondition.
    let uid = unsafe { libc::getuid() }.to_string();
    let Ok(out) = Command::new("ps").args(["-U", &uid, "-o", "pid=,rss=,%cpu=,comm="]).output() else { return Vec::new() };
    let me = std::process::id();
    let mut list: Vec<(u32, String, u64, f32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid: u32 = parts.next()?.parse().ok()?;
            let rss_kib: u64 = parts.next()?.parse().ok()?;
            let cpu: f32 = parts.next()?.replace(',', ".").parse().ok()?;
            let comm = parts.collect::<Vec<_>>().join(" ");
            let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
            Some((pid, name, rss_kib * 1024, cpu))
        })
        .filter(|p| p.0 != me)
        .collect();
    list.sort_by_key(|p| std::cmp::Reverse(p.2));
    list.truncate(CONTEXT_TOP);
    list
}

/// The instructions: the AI only picks actions from the registry, and never runs anything.
fn system_prompt() -> String {
    format!(
        "You are the assistant of mac-anal, a macOS app that analyses the user's Mac. You never run \
         anything and you never write commands. You answer the user's question with a short message \
         and a plan made only of actions from this list, filled with names, labels or pids that \
         appear in the context you are given:\n{}\nPropose an action only when it helps with the \
         question; an empty plan is a fine answer. Never quit a system process or one the user is \
         likely working in without saying why. Write `message` and every `reason` in French, \
         in one or two plain sentences.",
        actions::catalog()
    )
}

fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "message": {"type": "string"},
            "actions": {
                "type": "array",
                "maxItems": MAX_ACTIONS,
                "items": {
                    "type": "object",
                    "properties": {"reason": {"type": "string"}, "action": actions::schema()},
                    "required": ["reason", "action"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["message", "actions"],
        "additionalProperties": false,
    })
}

fn request(question: &str, context: &Context) -> ai::Request {
    ai::Request {
        system: system_prompt(),
        prompt: format!("Question: {question}\n\nContext (JSON):\n{}", context.to_json()),
        schema: answer_schema(),
    }
}

/// The answer, validated: every proposed action is either prepared or refused, never run.
pub struct Plan {
    pub message: String,
    pub items: Vec<Item>,
}

pub struct Item {
    pub reason: String,
    pub state: State,
}

pub enum State {
    /// Not in the registry, badly filled, or refused by `prepare`: shown, never run.
    Refused { what: String, why: String },
    Ready(Prepared),
    /// An irreversible action waiting for the explicit confirmation.
    Confirming(Prepared),
    Done(Prepared, actions::Done),
    Undone(Prepared),
    Failed(Prepared, String),
}

impl Plan {
    /// Checks the AI's JSON against the registry and the Mac.
    pub fn validate(env: &Env, answer: &Value) -> Result<Plan, String> {
        let message = answer.get("message").and_then(Value::as_str).ok_or("La réponse n'a pas de message.")?;
        let proposed = answer.get("actions").and_then(Value::as_array).ok_or("La réponse n'a pas de liste d'actions.")?;
        let items = proposed
            .iter()
            .take(MAX_ACTIONS)
            .map(|p| {
                let reason = p.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
                let raw = p.get("action").cloned().unwrap_or(Value::Null);
                let state = match actions::parse(&raw).and_then(|a| actions::prepare(env, a)) {
                    Ok(prepared) => State::Ready(prepared),
                    Err(why) => State::Refused { what: raw.to_string(), why },
                };
                Item { reason, state }
            })
            .collect();
        Ok(Plan { message: message.to_string(), items })
    }
}

enum Status {
    Idle,
    Thinking(Receiver<Result<Plan, String>>),
    Answered(Plan),
    Failed(String),
}

pub struct Assistant {
    question: String,
    status: Status,
}

impl Default for Assistant {
    fn default() -> Self {
        Self { question: String::new(), status: Status::Idle }
    }
}

impl Assistant {
    /// For the screenshot mode: with `MAC_ANAL_ASSISTANT=1`, an invented plan; nothing is read on the Mac.
    pub fn for_screenshot() -> Self {
        if std::env::var("MAC_ANAL_ASSISTANT").as_deref() == Ok("1") {
            Self::sample()
        } else {
            Self::default()
        }
    }

    fn sample() -> Self {
        let prepared = |action: Action, title: &str, preview: &str| Prepared::sample(action, title, preview);
        let cache = prepared(
            Action::ClearCache { name: "com.spotify.client".into() },
            "Vider le cache com.spotify.client",
            "Envoie ~/Library/Caches/com.spotify.client à la Corbeille : 18412 fichiers, 4,2 Go libérés une fois la Corbeille vidée.",
        );
        let agent = prepared(
            Action::DisableLoginItem { label: "com.example.updater".into() },
            "Désactiver com.example.updater au démarrage",
            "Met ~/Library/LaunchAgents/com.example.updater.plist de côté : il ne se lancera plus à la prochaine ouverture de session.",
        );
        let quit = prepared(
            Action::QuitProcess { pid: 4242 },
            "Quitter Google Chrome Helper",
            "Demande au processus 4242 (Google Chrome Helper) de quitter. Le travail qu'il n'a pas enregistré peut être perdu ; cela ne s'annule pas.",
        );
        let done = actions::Done::sample("com.example.updater ne se lancera plus au démarrage.");
        let items = vec![
            Item { reason: "Le plus gros cache, recréé au besoin par Spotify.".into(), state: State::Ready(cache) },
            Item { reason: "Ce processus garde 3,1 Go de mémoire depuis hier.".into(), state: State::Confirming(quit) },
            Item { reason: "Un programme de mise à jour qui se relance à chaque session.".into(), state: State::Done(agent, done) },
            Item {
                reason: "Libérer de la place.".into(),
                state: State::Refused {
                    what: r#"{"kind":"run_shell","command":"rm -rf ~/Downloads"}"#.into(),
                    why: "action inconnue « run_shell »".into(),
                },
            },
        ];
        Self {
            question: "Mon Mac est lent et le disque est presque plein.".into(),
            status: Status::Answered(Plan {
                message: "Trois pistes : un gros cache, un processus qui garde beaucoup de mémoire et un élément de démarrage inutile.".into(),
                items,
            }),
        }
    }

    fn ask(&mut self) {
        let question = self.question.trim().to_string();
        let Some(env) = Env::current() else {
            self.status = Status::Failed("Dossier personnel introuvable (HOME).".into());
            return;
        };
        let read_env = env.clone();
        let rx = ai::spawn(
            ai::command(),
            move || Ok(request(&question, &Context::read(&read_env))),
            move |answer| answer.and_then(|v| Plan::validate(&env, &v)),
        );
        self.status = Status::Thinking(rx);
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Status::Thinking(rx) = &self.status {
            match rx.try_recv() {
                Ok(Ok(plan)) => self.status = Status::Answered(plan),
                Ok(Err(e)) => self.status = Status::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.status = Status::Failed("L'appel à Claude s'est arrêté.".into()),
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(Duration::from_millis(200)),
            }
        }

        ui.add_space(8.0);
        ui.heading("Assistant");
        ui.add_space(8.0);
        let thinking = matches!(self.status, Status::Thinking(_));
        ui.add_enabled(
            !thinking,
            egui::TextEdit::multiline(&mut self.question).hint_text("Que voulez-vous améliorer ?").desired_rows(2).desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui.add_enabled(!thinking && !self.question.trim().is_empty(), egui::Button::new("Demander")).clicked() {
                self.ask();
            }
            if thinking {
                ui.spinner();
                ui.label("Claude réfléchit…");
                if ui.button("Abandonner").clicked() {
                    self.status = Status::Idle;
                }
            }
        });
        ui.add_space(8.0);

        match &mut self.status {
            Status::Idle | Status::Thinking(_) => {
                ui.label(RichText::new("L'assistant propose des actions ; rien ne se lance sans votre clic.").weak());
            }
            Status::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e.as_str());
            }
            Status::Answered(plan) => {
                ui.label(plan.message.as_str());
                ui.add_space(8.0);
                if plan.items.is_empty() {
                    ui.label(RichText::new("Aucune action proposée.").weak());
                }
                egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    for item in &mut plan.items {
                        show_item(ui, item);
                        ui.add_space(8.0);
                    }
                });
            }
        }
    }
}

fn level_color(ui: &egui::Ui, level: Level) -> Color32 {
    let dark = ui.visuals().dark_mode;
    match (level, dark) {
        (Level::Read, _) => ui.visuals().weak_text_color(),
        (Level::Reversible, true) => Color32::from_rgb(110, 190, 255),
        (Level::Reversible, false) => Color32::from_rgb(0, 100, 200),
        (Level::Irreversible, _) => ui.visuals().error_fg_color,
    }
}

/// One proposed action: title and level, why, what it touches, then what can be done with it.
fn show_item(ui: &mut egui::Ui, item: &mut Item) {
    let visuals = ui.visuals().clone();
    Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let (title, level) = match &item.state {
                State::Refused { .. } => ("Action refusée".to_string(), None),
                State::Ready(p) | State::Confirming(p) | State::Done(p, _) | State::Undone(p) | State::Failed(p, _) => {
                    (p.title.clone(), Some(p.level))
                }
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(title).strong());
                if let Some(level) = level {
                    ui.label(RichText::new(level.label()).small().color(level_color(ui, level)));
                }
            });
            if !item.reason.is_empty() {
                ui.label(RichText::new(item.reason.as_str()).italics());
            }
            ui.add_space(4.0);

            let mut next = None;
            match &item.state {
                State::Refused { what, why } => {
                    ui.colored_label(visuals.error_fg_color, format!("Refusée : {why}. Rien ne sera exécuté."));
                    ui.label(RichText::new(what.as_str()).monospace().small().weak());
                }
                State::Ready(p) => {
                    ui.label(p.preview.as_str());
                    let label = if p.level == Level::Irreversible { "Lancer…" } else { "Lancer" };
                    if ui.button(label).clicked() {
                        next = Some(if p.level == Level::Irreversible { State::Confirming(p.clone()) } else { run(p, false) });
                    }
                }
                State::Confirming(p) => {
                    ui.label(p.preview.as_str());
                    ui.colored_label(visuals.error_fg_color, "Action irréversible : confirmez-vous ?");
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Confirmer").color(visuals.error_fg_color)).clicked() {
                            next = Some(run(p, true));
                        }
                        if ui.button("Renoncer").clicked() {
                            next = Some(State::Ready(p.clone()));
                        }
                    });
                }
                State::Done(p, done) => {
                    ui.label(format!("Fait : {}", done.summary));
                    if let Some(undo) = &done.undo {
                        if ui.button("Annuler").clicked() {
                            next = Some(match actions::undo(undo) {
                                Ok(()) => State::Undone(p.clone()),
                                Err(e) => State::Failed(p.clone(), e),
                            });
                        }
                    }
                }
                State::Undone(_) => {
                    ui.label(RichText::new("Annulée : tout est remis en place.").weak());
                }
                State::Failed(_, e) => {
                    ui.colored_label(visuals.error_fg_color, e.as_str());
                }
            }
            if let Some(state) = next {
                item.state = state;
            }
        });
}

/// Runs one action from a click. A rename or a signal: quick enough for the window's thread.
fn run(prepared: &Prepared, confirmed: bool) -> State {
    let result = Env::current().ok_or_else(|| "Dossier personnel introuvable (HOME).".to_string()).and_then(|env| actions::execute(&env, prepared, confirmed));
    match result {
        Ok(done) => State::Done(prepared.clone(), done),
        Err(e) => State::Failed(prepared.clone(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::tests::home;

    #[test]
    fn a_plan_keeps_known_actions_and_refuses_the_rest() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("com.example.app")).unwrap();
        let answer = json!({
            "message": "Voici.",
            "actions": [
                {"reason": "gros", "action": {"kind": "clear_cache", "name": "com.example.app"}},
                {"reason": "vite", "action": {"kind": "run_shell", "command": "rm -rf ~"}},
                {"reason": "absent", "action": {"kind": "clear_cache", "name": "missing"}},
                {"reason": "évasion", "action": {"kind": "clear_cache", "name": "../../Documents"}},
                {"reason": "champ", "action": {"kind": "quit_process", "pid": 42, "signal": 9}},
            ],
        });
        let plan = Plan::validate(&env, &answer).unwrap();
        assert_eq!(plan.message, "Voici.");
        assert!(matches!(&plan.items[0].state, State::Ready(p) if p.level == Level::Reversible));
        for item in &plan.items[1..] {
            assert!(matches!(item.state, State::Refused { .. }), "{}", item.reason);
        }
        // Nothing was run while validating.
        assert!(env.caches().join("com.example.app").is_dir());
    }

    #[test]
    fn a_malformed_answer_is_an_error() {
        let (_d, env) = home();
        assert!(Plan::validate(&env, &json!({"actions": []})).is_err());
        assert!(Plan::validate(&env, &json!({"message": "x"})).is_err());
        let many = json!({"message": "x", "actions": vec![json!({"reason": "", "action": {}}); 50]});
        assert_eq!(Plan::validate(&env, &many).unwrap().items.len(), MAX_ACTIONS);
    }

    #[test]
    fn the_context_lists_caches_and_login_items_of_the_home() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("small")).unwrap();
        std::fs::create_dir_all(env.caches().join("big")).unwrap();
        std::fs::write(env.caches().join("big/blob"), vec![0u8; 4096]).unwrap();
        std::fs::create_dir_all(env.launch_agents()).unwrap();
        std::fs::write(env.launch_agents().join("com.example.agent.plist"), "").unwrap();
        let ctx = Context::read(&env);
        assert_eq!(ctx.caches[0], ("big".to_string(), 4096));
        assert_eq!(ctx.login_items, vec!["com.example.agent".to_string()]);
        assert!(ctx.processes.iter().all(|p| p.0 != std::process::id()));
        let v = ctx.to_json();
        assert_eq!(v["caches"][0]["name"], "big");
    }

    #[test]
    fn the_request_carries_the_registry_and_the_schema() {
        let req = request("Disque plein", &Context::default());
        assert!(req.system.contains("clear_cache") && req.system.contains("disable_login_item"));
        assert!(req.prompt.contains("Disque plein"));
        assert_eq!(req.schema["properties"]["actions"]["items"]["properties"]["action"], actions::schema());
    }

    #[test]
    fn the_sample_shows_every_state_a_screenshot_needs() {
        let a = Assistant::sample();
        let Status::Answered(plan) = a.status else { panic!("no plan") };
        assert!(plan.items.iter().any(|i| matches!(i.state, State::Confirming(_))));
        assert!(plan.items.iter().any(|i| matches!(i.state, State::Refused { .. })));
    }

    #[test]
    fn a_stand_in_claude_gives_a_validated_plan() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("com.example.app")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude");
        let reply = r#"{"is_error":false,"structured_output":{"message":"ok","actions":[{"reason":"r","action":{"kind":"clear_cache","name":"com.example.app"}},{"reason":"r","action":{"kind":"sudo","command":"x"}}]}}"#;
        std::fs::write(&script, format!("#!/bin/sh\necho '{reply}'\n")).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let read_env = env.clone();
        let rx = ai::spawn(
            script.to_string_lossy().into_owned(),
            move || Ok(request("q", &Context::read(&read_env))),
            move |answer| answer.and_then(|v| Plan::validate(&env, &v)),
        );
        let plan = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert!(matches!(plan.items[0].state, State::Ready(_)));
        assert!(matches!(plan.items[1].state, State::Refused { .. }));
    }

    /// Asks the real `claude` about a temporary home (about a cent); the plan is validated, never run.
    #[test]
    #[ignore]
    fn the_real_claude_proposes_actions_from_the_registry() {
        let (_d, env) = home();
        let cache = env.caches().join("com.example.bigcache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("blob"), vec![0u8; 1 << 20]).unwrap();
        let ctx = Context { caches: vec![("com.example.bigcache".into(), 9_000_000_000)], ..Default::default() };
        let answer = ai::ask(&ai::command(), &request("Mon disque est plein, que puis-je vider ?", &ctx)).unwrap();
        eprintln!("{answer:#}");
        let plan = Plan::validate(&env, &answer).unwrap();
        assert!(!plan.message.is_empty());
        assert!(plan.items.iter().any(|i| matches!(&i.state, State::Ready(p) if p.action == Action::ClearCache { name: "com.example.bigcache".into() })));
        assert!(cache.is_dir(), "validating ran nothing");
    }
}
