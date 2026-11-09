use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use egui::ThemePreference;

use crate::actions::Env;
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
            if let Some(env) = env {
                menubar::watch_largest(env);
            }
            readings = Some(rx);
        }
        Self {
            overview,
            assistant: Assistant::new(sample_plan),
            readings,
            settings,
            settings_path,
            settings_error: None,
            screenshot,
            frame: 0,
            shot_requested: false,
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
