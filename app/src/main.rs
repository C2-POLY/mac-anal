mod actions;
mod ai;
mod app;
mod assistant;
mod brand;
mod dashboard;
mod investigate;
mod journal;
mod launchd;
mod mcp;
mod menubar;
mod metrics;
mod overview;
mod rules;
mod script_card;
mod scripts;
mod settings;
mod tasks;
mod theme;

use std::path::PathBuf;

/// The Dock icon, drawn from `assets/icon-legacy.svg` (see the `icon_png_is_current` test).
const ICON: &[u8] = include_bytes!("../assets/icon.png");

fn icon() -> egui::IconData {
    let img = image::load_from_memory_with_format(ICON, image::ImageFormat::Png).expect("assets/icon.png is a PNG").into_rgba8();
    egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
}

fn main() -> eframe::Result<()> {
    // `mac-anal check`, run by launchd: the surveillance rules, without the window.
    if std::env::args().nth(1).as_deref() == Some("check") {
        std::process::exit(rules::run_check());
    }
    // `mac-anal mcp`: the MCP server on stdio, for Claude Desktop and Claude Code.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        std::process::exit(mcp::serve());
    }
    // `mac-anal run-task <id>`, run by launchd at a scheduled task's time.
    if std::env::args().nth(1).as_deref() == Some("run-task") {
        std::process::exit(tasks::run_task_command(std::env::args().nth(2).as_deref()));
    }
    let mut screenshot: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
        }
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title("Mac Anal")
        .with_icon(icon())
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([720.0, 480.0]);
    // wgpu (Metal): the OpenGL renderer shows stale or stretched frames while the window is resized.
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, ..Default::default() };
    eframe::run_native("mac-anal", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, screenshot)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_icon_is_a_1024_square() {
        let i = icon();
        assert_eq!((i.width, i.height), (1024, 1024));
        // The yellow square of icon-legacy.svg, inside a transparent margin of 100 px.
        let at = |x: usize, y: usize| &i.rgba[(y * 1024 + x) * 4..][..4];
        assert_eq!(at(10, 10)[3], 0);
        assert_eq!(at(300, 200), [0xFF, 0xD4, 0x3B, 0xFF]);
    }

    /// `cargo test -- --ignored icon_png_is_current` draws `assets/icon.png` again from its SVG.
    #[test]
    #[ignore]
    fn icon_png_is_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let svg = std::fs::read(dir.join("icon-legacy.svg")).unwrap();
        let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(1024, 1024).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::default(), &mut pixmap.as_mut());
        pixmap.save_png(dir.join("icon.png")).unwrap();
    }
}
