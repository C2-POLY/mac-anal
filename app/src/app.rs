use std::path::PathBuf;

use egui::ThemePreference;

use crate::assistant::Assistant;
use crate::overview::Overview;

/// Frames drawn before the screenshot is taken, so fonts and layout have settled.
const SHOT_FRAME: u32 = 30;
/// Frames after which a screenshot that never came back gives up.
const SHOT_TIMEOUT: u32 = 600;

pub struct App {
    overview: Overview,
    assistant: Assistant,
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
        let assistant = if screenshot.is_some() { Assistant::for_screenshot() } else { Assistant::default() };
        Self { overview, assistant, screenshot, frame: 0, shot_requested: false }
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
        egui::CentralPanel::default().show(root, |ui| self.overview.show(ui));
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
