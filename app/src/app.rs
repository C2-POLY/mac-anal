use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use egui::ThemePreference;

use crate::actions::Env;
use crate::journal::{self, Entry};
use crate::rules::{self, Alert, Rule};
use crate::tasks::{self, LastRun, Task};
use crate::assistant::Assistant;
use crate::overview::Overview;
use crate::settings::{Settings, Widget};
use crate::{menubar, metrics};

/// Frames drawn before the screenshot is taken, so fonts and layout have settled.
const SHOT_FRAME: u32 = 30;
/// Frames after which a screenshot that never came back gives up.
const SHOT_TIMEOUT: u32 = 600;

pub struct App {
    overview: Overview,
    assistant: Assistant,
    /// The readings of the metrics thread; none in the screenshot mode.
    readings: Option<Receiver<Overview>>,
    settings: Settings,
    settings_path: Option<PathBuf>,
    settings_error: Option<String>,
    env: Option<Env>,
    /// The journal as last read, with the generation it was read at.
    journal: (u64, Vec<Entry>),
    journal_error: Option<String>,
    /// Surveillance: the sentence typed, Claude's proposal, the rules and alerts as last read.
    watch_sentence: String,
    watch_pending: Option<Receiver<Result<Rule, String>>>,
    watch_proposal: Option<Rule>,
    watch: (u64, Vec<Rule>, Vec<Alert>),
    watch_read_at: Option<std::time::Instant>,
    watch_error: Option<String>,
    /// Scheduled tasks: the sentence, Claude's proposal with what it will do, the tasks as last read.
    task_sentence: String,
    task_pending: Option<Receiver<Result<Task, String>>>,
    task_proposal: Option<(Task, Result<String, String>)>,
    tasks: (u64, Vec<Task>, std::collections::BTreeMap<u64, LastRun>),
    tasks_read_at: Option<std::time::Instant>,
    task_error: Option<String>,
    /// The script library as last read, with the generation it was read at.
    library: (u64, Vec<crate::scripts::Saved>),
    /// `--screenshot <file>`: draw the sample data, save the window there, then quit.
    screenshot: Option<PathBuf>,
    frame: u32,
    shot_requested: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, screenshot: Option<PathBuf>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        cc.egui_ctx.set_theme(appearance(std::env::var("MAC_ANAL_APPEARANCE").ok().as_deref()));
        // The screenshot mode reads nothing on the Mac: it shows invented values.
        let overview = if screenshot.is_some() { Overview::sample() } else { Overview::default() };
        // MAC_ANAL_ASSISTANT=1 shows an invented plan in a screenshot.
        let sample_plan = screenshot.is_some() && std::env::var("MAC_ANAL_ASSISTANT").is_ok_and(|v| v == "1");
        let env = Env::current();
        let settings_path = env.as_ref().map(Settings::path);
        // MAC_ANAL_SETTINGS=<widgets> (for example `disk,cpu`) shows a choice in a screenshot.
        let settings = match (&screenshot, std::env::var("MAC_ANAL_SETTINGS")) {
            (Some(_), Ok(v)) => Settings { widgets: v.split(',').filter_map(|w| serde_json::from_value(serde_json::json!(w.trim())).ok()).collect() },
            (Some(_), Err(_)) => Settings::default(),
            (None, _) => settings_path.as_deref().map(Settings::load).unwrap_or_default(),
        };
        let env_kept = env.clone();
        let mut readings = None;
        if screenshot.is_none() {
            menubar::install(cc.egui_ctx.clone(), env.clone(), &settings);
            let (tx, rx) = mpsc::channel();
            let ctx = cc.egui_ctx.clone();
            metrics::spawn(move |o| {
                let _ = tx.send(o);
                ctx.request_repaint();
                menubar::post(o);
            });
            if let Some(env) = env.clone() {
                menubar::watch_largest(env);
            }
            readings = Some(rx);
        }
        let mut assistant = Assistant::new(sample_plan);
        if screenshot.is_some() && std::env::var("MAC_ANAL_SCRIPT").is_ok_and(|v| v == "1") {
            assistant.sample_script();
        }
        Self {
            overview,
            assistant,
            readings,
            settings,
            settings_path,
            settings_error: None,
            journal: (u64::MAX, Vec::new()),
            journal_error: None,
            watch_sentence: String::new(),
            watch_pending: None,
            watch_proposal: None,
            watch: (u64::MAX, Vec::new(), Vec::new()),
            watch_read_at: None,
            watch_error: None,
            task_sentence: String::new(),
            task_pending: None,
            task_proposal: None,
            tasks: (u64::MAX, Vec::new(), Default::default()),
            tasks_read_at: None,
            task_error: None,
            library: (u64::MAX, Vec::new()),
            env: env_kept,
            screenshot,
            frame: 0,
            shot_requested: false,
        }
    }

    /// The kept scripts: open one in the assistant to read it or run it again, or remove it.
    fn scripts_ui(&mut self, ui: &mut egui::Ui) {
        use crate::scripts;
        let Some(env) = self.env.clone() else { return };
        if self.screenshot.is_some() {
            if self.library.0 == u64::MAX {
                let sample = std::env::var("MAC_ANAL_SCRIPT").is_ok_and(|v| v == "1");
                let saved = |name: &str, purpose: &str| scripts::Saved { name: name.into(), purpose: purpose.into(), script: String::new(), created: 0 };
                self.library = (0, if sample { vec![saved("lister-gros-fichiers", "Lister les gros fichiers des Téléchargements"), saved("taille-xcode", "Mesurer les données de Xcode")] } else { Vec::new() });
            }
        } else if self.library.0 != scripts::generation() {
            self.library = (scripts::generation(), scripts::list(&env));
        }
        ui.add_space(24.0);
        ui.heading("Scripts");
        ui.add_space(4.0);
        if self.library.1.is_empty() {
            ui.label(egui::RichText::new("Aucun script enregistré : « Écrire un script » dans l'assistant, puis « Enregistrer ».").weak());
            return;
        }
        let mut open = None;
        let mut remove = None;
        for s in &self.library.1 {
            ui.horizontal(|ui| {
                ui.label(&s.purpose);
                if ui.small_button("Ouvrir").on_hover_text("Le relire, l'essayer à blanc ou le relancer").clicked() {
                    open = Some(s.clone());
                }
                if ui.small_button("Supprimer").on_hover_text("Il part dans la Corbeille").clicked() {
                    remove = Some(s.name.clone());
                }
            });
        }
        if let Some(s) = open {
            self.assistant.load_script(s);
        }
        if let Some(name) = remove {
            if let Err(e) = scripts::delete(&env, &name) {
                self.journal_error = Some(e);
            }
        }
    }

    /// The last actions, newest first, with their effect and an undo for the reversible ones.
    /// Rules stated in a sentence, checked every minute by `mac-anal check` from launchd.
    fn surveillance_ui(&mut self, ui: &mut egui::Ui) {
        let Some(env) = self.env.clone() else { return };
        let live = self.screenshot.is_none();
        // Read again when the app changed the rules, and every few seconds for the alerts that
        // `mac-anal check` writes from its own process.
        if live {
            let stale = self.watch_read_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5));
            if self.watch.0 != rules::generation() || stale {
                self.watch = (rules::generation(), rules::rules(&env), rules::alerts(&env));
                self.watch_read_at = Some(std::time::Instant::now());
            }
        } else if self.watch.0 == u64::MAX {
            self.watch = (0, Vec::new(), Vec::new());
            if std::env::var("MAC_ANAL_WATCH").is_ok_and(|v| v == "1") {
                let (r, a, p) = rules::sample();
                self.watch = (0, r, a);
                self.watch_sentence = "préviens-moi si la mémoire dépasse 90 %".into();
                self.watch_proposal = Some(p);
            }
        }
        if let Some(rx) = &self.watch_pending {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(rule) => self.watch_proposal = Some(rule),
                    Err(e) => self.watch_error = Some(e),
                }
                self.watch_pending = None;
            }
        }

        ui.add_space(24.0);
        ui.heading("Surveillance");
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Vérifiée chaque minute en arrière-plan par launchd, sans réseau ni IA, même Mac Anal fermé.").weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.watch_sentence).hint_text("Ex. : préviens-moi si le disque passe sous 10 %").desired_width(360.0));
            let ready = self.watch_pending.is_none() && !self.watch_sentence.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Créer la règle")).clicked() {
                let (tx, rx) = mpsc::channel();
                let sentence = self.watch_sentence.trim().to_string();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let _ = tx.send(rules::from_sentence(&crate::ai::command(), &sentence));
                    ctx.request_repaint();
                });
                self.watch_pending = Some(rx);
                self.watch_error = None;
            }
            if self.watch_pending.is_some() {
                ui.spinner();
            }
        });

        let mut changed = false;
        let mut save_proposal = false;
        let mut drop_proposal = false;
        if let Some(p) = &mut self.watch_proposal {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("Règle proposée : {}", p.name)).strong());
            ui.label(egui::RichText::new(p.condition()).weak());
            ui.horizontal(|ui| {
                ui.label("Seuil");
                ui.add(egui::DragValue::new(&mut p.threshold).speed(0.5));
                ui.label("toutes les");
                ui.add(egui::DragValue::new(&mut p.every_minutes).range(1..=1440));
                ui.label("min");
                if ui.button("Enregistrer").clicked() {
                    save_proposal = true;
                }
                if ui.button("Annuler").clicked() {
                    drop_proposal = true;
                }
            });
        }
        if drop_proposal {
            self.watch_proposal = None;
        }
        if save_proposal {
            if let Some(p) = self.watch_proposal.take() {
                match p.check().and_then(|p| rules::add(&env, p)) {
                    Ok(_) => {
                        self.watch_sentence.clear();
                        changed = true;
                    }
                    Err(e) => self.watch_error = Some(e),
                }
            }
        }

        let mut remove = None;
        let mut edited = false;
        for r in &mut self.watch.1 {
            ui.horizontal(|ui| {
                let mut active = !r.paused;
                if ui.checkbox(&mut active, "").on_hover_text("Active ou en pause").changed() {
                    r.paused = !active;
                    edited = true;
                }
                ui.label(egui::RichText::new(&r.name).strong());
                ui.label(egui::RichText::new(r.condition()).weak());
                if ui.add(egui::DragValue::new(&mut r.threshold).speed(0.5)).on_hover_text("Seuil").changed() {
                    edited = true;
                }
                if ui.small_button("Supprimer").clicked() {
                    remove = Some(r.id);
                }
            });
        }
        if live && (edited || remove.is_some()) {
            let kept: Vec<Rule> = self.watch.1.iter().filter(|r| Some(r.id) != remove).cloned().collect();
            if let Err(e) = kept.iter().cloned().map(Rule::check).collect::<Result<Vec<_>, _>>().and_then(|k| rules::save_rules(&env, &k)) {
                self.watch_error = Some(e);
            }
            changed = true;
        }
        if live && changed {
            if let Err(e) = rules::sync_agent(&env, crate::launchd::LAUNCHCTL) {
                self.watch_error = Some(format!("launchd : {e}"));
            }
        }

        if !self.watch.2.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Dernières alertes").strong());
            let mut explain = None;
            for a in self.watch.2.iter().rev().take(5) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(journal::date(a.at)).weak().monospace());
                    ui.label(&a.message);
                    if ui.small_button("Expliquer").clicked() {
                        explain = Some(a.message.clone());
                    }
                });
            }
            if let Some(m) = explain {
                self.assistant.explain(ui.ctx(), format!("Explique cette alerte de surveillance et propose des actions : {m}"));
            }
        }
        if let Some(e) = &self.watch_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    /// Recurring tasks stated in a sentence, each a launchd agent; only reversible actions and
    /// read-only scripts.
    fn tasks_ui(&mut self, ui: &mut egui::Ui) {
        let Some(env) = self.env.clone() else { return };
        let live = self.screenshot.is_none();
        if live {
            // The last runs are written by `mac-anal run-task`, another process: read them now and then.
            let stale = self.tasks_read_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5));
            if self.tasks.0 != tasks::generation() || stale {
                self.tasks = (tasks::generation(), tasks::tasks(&env), tasks::last_runs(&env));
                self.tasks_read_at = Some(std::time::Instant::now());
            }
        } else if self.tasks.0 == u64::MAX {
            self.tasks.0 = 0;
            if std::env::var("MAC_ANAL_TASKS").is_ok_and(|v| v == "1") {
                let (t, r, p) = tasks::sample();
                self.tasks = (0, t, r);
                self.task_sentence = "chaque vendredi à 17 h, mets à la Corbeille les téléchargements de plus de 90 jours".into();
                self.task_proposal = Some((p, Ok("37 éléments de ~/Downloads inchangés depuis 90 jours (4,2 Go) partent à la Corbeille.".into())));
            }
        }
        if let Some(rx) = &self.task_pending {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(t) => {
                        let check = t.check(&env);
                        self.task_proposal = Some((t, check));
                    }
                    Err(e) => self.task_error = Some(e),
                }
                self.task_pending = None;
            }
        }

        ui.add_space(24.0);
        ui.heading("Tâches planifiées");
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Lancées par launchd à leur heure, même Mac Anal fermé : actions réversibles et scripts en lecture seule uniquement.").weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.task_sentence).hint_text("Ex. : chaque lundi, vide les DerivedData de plus de 30 jours").desired_width(360.0));
            let ready = self.task_pending.is_none() && !self.task_sentence.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Préparer la tâche")).clicked() {
                let (tx, rx) = mpsc::channel();
                let sentence = self.task_sentence.trim().to_string();
                let (ctx, env) = (ui.ctx().clone(), env.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(tasks::from_sentence(&crate::ai::command(), &env, &sentence));
                    ctx.request_repaint();
                });
                self.task_pending = Some(rx);
                self.task_error = None;
            }
            if self.task_pending.is_some() {
                ui.spinner();
            }
        });

        let mut decided = None;
        if let Some((t, check)) = &self.task_proposal {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("Tâche proposée : {} ({})", t.name, t.schedule())).strong());
            match check {
                Ok(what) => {
                    ui.label(egui::RichText::new(what).weak());
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            }
            if let Ok(plist) = t.plist() {
                egui::CollapsingHeader::new("Le plist chargé dans launchd").id_salt("plist").show(ui, |ui| {
                    ui.label(egui::RichText::new(plist).monospace().small());
                });
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(check.is_ok(), egui::Button::new("Programmer")).clicked() {
                    decided = Some(true);
                }
                if ui.button("Annuler").clicked() {
                    decided = Some(false);
                }
            });
        }
        if let Some(go) = decided {
            if let Some((t, _)) = self.task_proposal.take() {
                if go && live {
                    match tasks::schedule(&env, crate::launchd::LAUNCHCTL, t) {
                        Ok(_) => self.task_sentence.clear(),
                        Err(e) => self.task_error = Some(e),
                    }
                }
            }
        }

        let now = journal::now();
        let mut toggle = None;
        let mut remove = None;
        for t in &self.tasks.1 {
            ui.horizontal_wrapped(|ui| {
                let mut active = !t.paused;
                if ui.checkbox(&mut active, "").on_hover_text("Active ou en pause").changed() {
                    toggle = Some((t.id, !active));
                }
                ui.label(egui::RichText::new(&t.name).strong());
                ui.label(egui::RichText::new(t.schedule()).weak());
                if !t.paused {
                    if let Some(next) = t.next_run(now) {
                        ui.label(egui::RichText::new(format!("prochaine : {}", journal::date(next))).weak());
                    }
                }
                if ui.small_button("Supprimer").clicked() {
                    remove = Some(t.id);
                }
            });
            if let Some(last) = self.tasks.2.get(&t.id) {
                let text = format!("Dernière exécution {} : {}", journal::date(last.at), last.summary);
                if last.ok {
                    ui.label(egui::RichText::new(text).weak());
                } else {
                    ui.colored_label(ui.visuals().error_fg_color, text);
                }
            }
        }
        if live {
            if let Some((id, paused)) = toggle {
                self.task_error = tasks::set_paused(&env, crate::launchd::LAUNCHCTL, id, paused).err();
            }
            if let Some(id) = remove {
                self.task_error = tasks::remove(&env, crate::launchd::LAUNCHCTL, id).err();
            }
        }
        if let Some(e) = &self.task_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    fn journal_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.heading("Journal");
        ui.add_space(4.0);
        let Some(env) = self.env.clone() else { return };
        // MAC_ANAL_JOURNAL=1 shows invented entries in a screenshot; otherwise nothing is read then.
        if self.screenshot.is_some() {
            if self.journal.0 == u64::MAX {
                self.journal = (0, if std::env::var("MAC_ANAL_JOURNAL").is_ok_and(|v| v == "1") { journal::sample() } else { Vec::new() });
            }
        } else if self.journal.0 != journal::generation() {
            self.journal = (journal::generation(), journal::load(&env));
        }
        if self.journal.1.is_empty() {
            ui.label(egui::RichText::new("Rien encore : les actions lancées depuis l'assistant ou la barre des menus s'inscriront ici.").weak());
            return;
        }
        let mut undo = None;
        for e in self.journal.1.iter().rev().take(20) {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(journal::date(e.at)).weak().monospace());
                ui.label(egui::RichText::new(&e.title).strong());
                ui.label(egui::RichText::new(e.origin.label()).small().weak());
                if e.undo.is_some() && !e.undone && e.error.is_none() && ui.small_button("Annuler").clicked() {
                    undo = Some(e.id);
                }
            });
            ui.label(egui::RichText::new(e.effect()).weak());
            ui.add_space(4.0);
        }
        if let Some(id) = undo {
            self.journal_error = journal::undo(&env, id).err();
        }
        if let Some(e) = &self.journal_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    /// Choose and order the menu bar widgets; saved and applied at once.
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.heading("Barre des menus");
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Les widgets restent à jour fenêtre fermée.").weak());
        ui.add_space(8.0);
        let mut changed = false;
        let order: Vec<Widget> = self.settings.widgets.iter().copied().chain(Widget::ALL.into_iter().filter(|w| !self.settings.widgets.contains(w))).collect();
        for w in order {
            let shown = self.settings.widgets.contains(&w);
            ui.horizontal(|ui| {
                let mut on = shown;
                if ui.checkbox(&mut on, w.name()).changed() {
                    self.settings.toggle(w);
                    changed = true;
                }
                if shown {
                    if ui.small_button("<").on_hover_text("Plus à gauche").clicked() {
                        self.settings.shift(w, -1);
                        changed = true;
                    }
                    if ui.small_button(">").on_hover_text("Plus à droite").clicked() {
                        self.settings.shift(w, 1);
                        changed = true;
                    }
                }
            });
        }
        if changed && self.readings.is_some() {
            menubar::rebuild(&self.settings);
            self.settings_error = self.settings_path.as_deref().and_then(|p| self.settings.save(p).err());
        }
        if let Some(e) = &self.settings_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frame += 1;
        ctx.request_repaint();
        if self.frame >= SHOT_FRAME && !self.shot_requested {
            // Saved and the window closed once the image is ready, from the renderer's callback.
            let done = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = image::save_buffer(&path, &bytes, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                    eprintln!("could not save screenshot: {e}");
                }
                done.send_viewport_cmd(egui::ViewportCommand::Close);
                done.request_repaint();
            })));
            self.shot_requested = true;
        } else if self.frame > SHOT_TIMEOUT {
            eprintln!("screenshot timed out");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// `MAC_ANAL_APPEARANCE=light|dark` forces a theme (for screenshots); anything else follows the system.
fn appearance(value: Option<&str>) -> ThemePreference {
    match value {
        Some("light") => ThemePreference::Light,
        Some("dark") => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        egui::Panel::right("assistant").default_size(400.0).min_size(320.0).show(root, |ui| self.assistant.show(ui));
        if let Some(rx) = &self.readings {
            if let Some(o) = rx.try_iter().last() {
                self.overview = o;
            }
        }
        // An alert clicked in the menu bar: the assistant explains it.
        if let Some(a) = menubar::take_explain() {
            self.assistant.explain(&ctx, format!("Explique cette alerte de surveillance et propose des actions : {a}"));
        }
        // With widgets in the menu bar, closing the window hides it; the menu quits.
        if ctx.input(|i| i.viewport().close_requested()) && menubar::active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                self.overview.show(ui);
                self.settings_ui(ui);
                self.surveillance_ui(ui);
                self.tasks_ui(ui);
                self.scripts_ui(ui);
                self.journal_ui(ui);
            });
        });
        self.screenshot_step(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_follows_the_system_unless_forced() {
        assert_eq!(appearance(Some("light")), ThemePreference::Light);
        assert_eq!(appearance(Some("dark")), ThemePreference::Dark);
        assert_eq!(appearance(Some("blue")), ThemePreference::System);
        assert_eq!(appearance(None), ThemePreference::System);
    }
}
