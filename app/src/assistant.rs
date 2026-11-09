//! The Assistant panel: a question, the investigation's steps as they come, then the answer with
//! its evidence and its plan of actions from the catalog, each with what it touches, its level, and
//! Run / Confirm / Undo. The app runs the tools and the actions, never the AI.

use std::process::Command;
use std::sync::mpsc::{self, Receiver};

use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use serde_json::Value;

use crate::actions::{self, Action, Done, Env, Level, Prepared};
use crate::ai;
use crate::investigate::{self, Progress};
use crate::script_card::ScriptCard;
use crate::scripts;
use crate::journal::{self, Origin};

/// What the AI proposed, checked by the app.
pub struct Plan {
    pub message: String,
    /// What the answer rests on: excerpts of tool outputs, names, sizes.
    pub evidence: Vec<String>,
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
    pending: Option<Receiver<Progress>>,
    /// The investigation's steps: what was read, and how many lines.
    steps: Vec<(String, usize)>,
    plan: Option<Plan>,
    error: Option<String>,
    /// A script being written by Claude: its purpose and code.
    writing: Option<Receiver<Result<(String, String), String>>>,
    script: Option<ScriptCard>,
}

impl Assistant {
    pub fn new(sample: bool) -> Self {
        let mut a = Assistant { env: Env::current(), question: String::new(), pending: None, steps: Vec::new(), plan: None, error: None, writing: None, script: None };
        if sample {
            a.question = "Libère de la place sur mon disque".into();
            a.steps = vec![("Taille des dossiers de ~/Library/Caches".into(), 41), ("Processus les plus actifs".into(), 312)];
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
            investigate::investigate(&ai::command(), &env, &question, &context(&env), |p| {
                let _ = tx.send(p);
                ctx.request_repaint();
            });
        });
        self.pending = Some(rx);
        self.steps.clear();
        self.plan = None;
        self.error = None;
    }

    /// Opens a kept script from the library in the card.
    pub fn load_script(&mut self, saved: scripts::Saved) {
        self.script = Some(ScriptCard::new(saved.purpose, saved.script, Some(saved.name)));
    }

    /// An invented script for the screenshot mode (`MAC_ANAL_SCRIPT=1`).
    pub fn sample_script(&mut self) {
        self.question = "Liste les dix plus gros fichiers de mes Téléchargements".into();
        self.script = Some(ScriptCard::new(
            "Lister les dix plus gros fichiers des Téléchargements".into(),
            "# Les dix plus gros fichiers, du plus gros au plus petit\nfind ~/Downloads -type f -size +10M -print0 | xargs -0 du -h | sort -rh | head -10\n".into(),
            None,
        ));
    }

    fn write_script(&mut self, ctx: &egui::Context) {
        let need = self.question.trim().to_string();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scripts::generate(&ai::command(), &need));
            ctx.request_repaint();
        });
        self.writing = Some(rx);
        self.script = None;
        self.error = None;
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.writing {
            match rx.try_recv() {
                Ok(Ok((purpose, script))) => {
                    self.script = Some(ScriptCard::new(purpose, script, None));
                    self.writing = None;
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.writing = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.writing = None,
            }
        }
        let Some(rx) = &self.pending else { return };
        loop {
            match rx.try_recv() {
                Ok(Progress::Step { label, lines }) => self.steps.push((label, lines)),
                Ok(Progress::Answer(v)) => {
                    match self.env.as_ref().ok_or("Dossier personnel introuvable (HOME).".to_string()).and_then(|env| plan_from(env, &v)) {
                        Ok(plan) => self.plan = Some(plan),
                        Err(e) => self.error = Some(e),
                    }
                    self.pending = None;
                    return;
                }
                Ok(Progress::Failed(e)) => {
                    self.error = Some(e);
                    self.pending = None;
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("L'analyse s'est arrêtée sans réponse.".into());
                    self.pending = None;
                    return;
                }
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
            let ready = !busy && self.writing.is_none() && !self.question.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Demander")).clicked() {
                self.ask(ui.ctx());
            }
            if ui.add_enabled(ready, egui::Button::new("Écrire un script")).on_hover_text("Pour ce que les actions connues ne couvrent pas.").clicked() {
                self.write_script(ui.ctx());
            }
            if self.writing.is_some() {
                ui.spinner();
                ui.label(RichText::new("Écriture du script…").weak());
            }
            if busy {
                ui.spinner();
                ui.label(RichText::new("Enquête en cours…").weak());
            }
        });
        ui.label(RichText::new("Ce que l'assistant lit sur le Mac (processus, journaux, tailles) est envoyé à Claude.").small().weak());
        if !self.steps.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Enquête").strong());
            for (label, lines) in &self.steps {
                ui.label(RichText::new(format!("• {label} ({lines} lignes lues)")).weak());
            }
        }
        if let Some(e) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let env = self.env.clone();
        if let Some(card) = &mut self.script {
            ui.add_space(12.0);
            let visuals = ui.visuals().clone();
            Frame::new()
                .fill(visuals.faint_bg_color)
                .stroke(Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    card.show(ui, env.as_ref());
                });
        }
        let Some(plan) = &mut self.plan else { return };
        ui.add_space(12.0);
        ui.label(&plan.message);
        if !plan.evidence.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Éléments relevés").strong());
            for e in &plan.evidence {
                ui.label(RichText::new(format!("• {e}")).weak());
            }
        }
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
                            let undone = match (env, done.entry) {
                                (Some(env), Some(id)) => journal::undo(env, id),
                                _ => actions::undo(u),
                            };
                            next = Some(match undone {
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
    match journal::run(env, Origin::Assistant, p, confirmed) {
        Ok(done) => State::Done(done),
        Err(e) => State::Failed(e),
    }
}

/// The AI's answer turned into a plan: an unknown action, or one that fails its checks, is kept
/// as refused, with why, and can never run.
pub fn plan_from(env: &Env, v: &Value) -> Result<Plan, String> {
    let message = v.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
    let evidence = v.get("evidence").and_then(Value::as_array).map(|a| a.iter().filter_map(|e| e.as_str().map(String::from)).collect()).unwrap_or_default();
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
    Ok(Plan { message, evidence, items })
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
    out.push_str("What mac-anal did lately (journal, newest first):\n");
    out.push_str(&journal::summary(env, 10));
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
        evidence: vec!["~/Library/Caches/com.example.music : 2,3 Go".into(), "Example Helper (4242) : 1,8 Go de mémoire".into()],
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
                state: State::Done(Done { summary: "com.example.updater ne se lancera plus au démarrage.".into(), undo: None, entry: None }),
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
    use serde_json::json;

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
    fn the_answer_schema_and_the_catalog_name_the_same_kinds() {
        let s = investigate::schema(true);
        let kinds: Vec<&str> = s["properties"]["step"]["anyOf"][0]["properties"]["answer"]["properties"]["actions"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["properties"]["kind"]["const"].as_str().unwrap())
            .collect();
        let catalog: Vec<&str> = crate::actions::CATALOG.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, catalog);
    }

    #[test]
    fn the_whole_loop_runs_with_a_stand_in_claude() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        std::fs::create_dir_all(env.caches().join("com.example.app")).unwrap();
        let script = dir.path().join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\necho '{\"is_error\":false,\"structured_output\":{\"step\":{\"answer\":{\"message\":\"Un cache\",\"evidence\":[\"2 Go\"],\"actions\":[{\"kind\":\"clear_cache\",\"name\":\"com.example.app\"}]}}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let answer = std::sync::Mutex::new(None);
        investigate::investigate(script.to_str().unwrap(), &env, "place", &context(&env), |p| {
            if let Progress::Answer(v) = p {
                *answer.lock().unwrap() = Some(v);
            }
        });
        let plan = plan_from(&env, &answer.into_inner().unwrap().unwrap()).unwrap();
        assert_eq!(plan.evidence, vec!["2 Go".to_string()]);
        let p = plan.items[0].proposal.as_ref().unwrap();
        let done = actions::execute(&env, p, false).unwrap();
        assert!(!env.caches().join("com.example.app").exists());
        actions::undo(done.undo.as_ref().unwrap()).unwrap();
        assert!(env.caches().join("com.example.app").is_dir());
    }
}
