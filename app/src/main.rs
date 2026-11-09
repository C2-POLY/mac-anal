mod actions;
mod ai;
mod app;
mod assistant;
mod investigate;
mod journal;
mod menubar;
mod metrics;
mod overview;
mod settings;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let mut screenshot: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
        }
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title("mac-anal")
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([720.0, 480.0]);
    // wgpu (Metal): the OpenGL renderer shows stale or stretched frames while the window is resized.
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, ..Default::default() };
    eframe::run_native("mac-anal", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, screenshot)))))
}
