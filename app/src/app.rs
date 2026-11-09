use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use egui::ThemePreference;

use crate::actions::Env;
use crate::journal::{self, Entry};
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
    /// The script library as last read, with the generation it was read at.
    library: (u64, Vec<crate::scripts::Saved>),
    /// `--screenshot <file>`: draw the sample data, save the window there, then quit.
    screenshot: Option<PathBuf>,
    frame: u32,
    shot_requested: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, screenshot: Option<PathBuf>) -> Self {
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
        // With widgets in the menu bar, closing the window hides it; the menu quits.
        if ctx.input(|i| i.viewport().close_requested()) && menubar::active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        egui::CentralPanel::default().show(root, |ui| {
            self.overview.show(ui);
            self.settings_ui(ui);
            self.scripts_ui(ui);
            self.journal_ui(ui);
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
