//! The widgets of the macOS menu bar: one `NSStatusItem` per chosen widget, showing a live value
//! and holding a menu with the detail, the alerts and quick reversible actions.
//!
//! Status items only live on the main thread, so they are kept in a thread-local there, and every
//! update reaches them through the main GCD queue. That keeps them fresh while the window is hidden,
//! without going through egui.

use std::cell::RefCell;
use std::sync::Mutex;

use dispatch2::DispatchQueue;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::actions::{self, Action, Env, Level, Prepared};
use crate::journal::{self, Origin};
use crate::dashboard::format::{decimal, duration, percent, rate, sensor_name};
use crate::dashboard::{Snapshot, SortBy, Thermal};
use crate::overview::format_bytes;
use crate::settings::{Settings, Widget};

const OPEN: &str = "open";
const QUIT: &str = "quit";
const CLEAR: &str = "clear-largest";
const UNDO: &str = "undo";
const ALERT: &str = "alert";

/// The alert the user clicked in a menu, for the window's assistant to explain.
static EXPLAIN: Mutex<Option<String>> = Mutex::new(None);

/// Takes the alert to explain, if one was clicked since.
pub fn take_explain() -> Option<String> {
    EXPLAIN.lock().ok()?.take()
}

/// What a widget shows before its first reading.
const WAITING: &str = "…";
/// Processes listed in the processor and memory menus.
const TOP: usize = 3;

/// The glyph of a widget, a PNG drawn from `assets/menubar/<name>.svg` (see the
/// `menubar_glyphs_are_current` test). Black on transparent: macOS tints it as a template image.
/// All are 20 × 22 points at 2×, the height of the menu bar, so every glyph sits in the same box
/// and lines up with the text beside it; the last 2 points are empty, a gap before the text.
pub fn glyph(w: Widget, s: &Snapshot) -> &'static [u8] {
    match w {
        Widget::Disk => include_bytes!("../assets/menubar/disk.png"),
        Widget::Memory => include_bytes!("../assets/menubar/memory.png"),
        Widget::Cpu => include_bytes!("../assets/menubar/cpu.png"),
        Widget::Gpu => include_bytes!("../assets/menubar/gpu.png"),
        Widget::Network => include_bytes!("../assets/menubar/network.png"),
        Widget::Temperature => include_bytes!("../assets/menubar/temperature.png"),
        Widget::Fan => include_bytes!("../assets/menubar/fan.png"),
        Widget::Battery if s.sensors.battery.as_ref().is_some_and(|b| b.charging) => include_bytes!("../assets/menubar/battery-charging.png"),
        Widget::Battery => include_bytes!("../assets/menubar/battery.png"),
        Widget::Swap => include_bytes!("../assets/menubar/swap.png"),
    }
}

fn image(png: &[u8]) -> Result<tray_icon::Icon, String> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
}

/// The text a widget shows in the menu bar, beside its glyph.
pub fn title(w: Widget, s: &Snapshot) -> String {
    let o = &s.overview;
    let value = match w {
        Widget::Disk => o.disk.map(|d| format_bytes(d.total.saturating_sub(d.used))),
        Widget::Memory => o.memory.map(|m| percent(m.used as f32 / m.total.max(1) as f32)),
        Widget::Cpu => o.cpu.map(percent),
        Widget::Gpu => s.gpus.iter().find_map(|g| g.usage).map(percent),
        Widget::Network => (!s.interfaces.is_empty()).then(|| {
            let (down, up) = s.network_rate();
            format!("↓{} ↑{}", rate(down), rate(up))
        }),
        Widget::Temperature => s.sensors.temperatures.first().map(|(_, t)| format!("{} °C", decimal(*t as f64, 0))),
        Widget::Fan => s.sensors.fans.first().map(|f| format!("{} tr/min", decimal(f.rpm as f64, 0))),
        Widget::Battery => s.sensors.battery.as_ref().map(|b| percent(b.charge)),
        Widget::Swap => s.memory.as_ref().map(|m| format_bytes(m.swap_used)),
    };
    value.unwrap_or_else(|| WAITING.to_string())
}

/// The lines at the top of a widget's menu: a summary, then the detail of the measure.
pub fn detail(w: Widget, s: &Snapshot) -> Vec<String> {
    let o = &s.overview;
    let waiting = || vec![format!("{} : mesure en cours", w.name())];
    match w {
        Widget::Disk => {
            let Some(d) = o.disk else { return waiting() };
            let mut lines = vec![format!("Disque : {} libres sur {}", format_bytes(d.total.saturating_sub(d.used)), format_bytes(d.total))];
            for v in &s.volumes {
                lines.push(format!("{} : {} libres sur {}", v.name, format_bytes(v.total.saturating_sub(v.used)), format_bytes(v.total)));
            }
            if !s.volumes.is_empty() {
                let (read, written) = s.disk_rate();
                lines.push(format!("Lecture {} · écriture {}", rate(read), rate(written)));
            }
            lines
        }
        Widget::Memory => {
            let Some(m) = o.memory else { return waiting() };
            let mut lines = vec![format!("Mémoire : {} utilisés sur {}", format_bytes(m.used), format_bytes(m.total))];
            if let Some(m) = &s.memory {
                lines.push(format!("Applications {} · câblée {} · compressée {}", format_bytes(m.app), format_bytes(m.wired), format_bytes(m.compressed)));
                lines.push(format!("Cache {} · libre {}", format_bytes(m.cached), format_bytes(m.free)));
                lines.push(format!("Swap {} sur {}", format_bytes(m.swap_used), format_bytes(m.swap_total)));
                lines.extend(pressure(m.pressure_free));
            }
            lines.extend(heaviest(s, SortBy::Memory));
            lines
        }
        Widget::Cpu => {
            let Some(c) = o.cpu else { return waiting() };
            let mut lines = vec![format!("Processeur : {:.0} % de charge", c * 100.0)];
            if let Some(c) = &s.cpu {
                lines.push(format!("Charge moyenne 1, 5, 15 min : {}", c.load.map(|l| decimal(l, 2)).join(" · ")));
                lines.push(format!("{} cœurs performance, {} efficacité", c.performance, c.efficiency));
            }
            lines.extend(heaviest(s, SortBy::Cpu));
            lines
        }
        Widget::Gpu => {
            if s.gpus.is_empty() {
                return waiting();
            }
            s.gpus.iter().map(|g| format!("GPU : {} ({})", g.usage.map_or("utilisation inconnue".into(), percent), g.name)).collect()
        }
        Widget::Network => {
            if s.interfaces.is_empty() {
                return waiting();
            }
            let (down, up) = s.network_rate();
            let mut lines = vec![format!("Réseau : ↓ {} ↑ {}", rate(down), rate(up))];
            for i in &s.interfaces {
                let name = i.address.as_ref().map_or(i.name.clone(), |a| format!("{} ({a})", i.name));
                lines.push(format!("{name} : ↓ {} ↑ {}", rate(i.received_per_s), rate(i.sent_per_s)));
            }
            lines
        }
        Widget::Temperature => {
            let Some((_, hottest)) = s.sensors.temperatures.first() else { return waiting() };
            let mut lines = vec![format!("Température : {} °C au plus chaud", decimal(*hottest as f64, 0))];
            for (name, t) in s.sensors.temperatures.iter().take(TOP) {
                lines.push(format!("{} : {} °C", sensor_name(name), decimal(*t as f64, 1)));
            }
            lines.extend(thermal(s.sensors.thermal));
            lines
        }
        Widget::Fan => {
            // Sensors read but no fan: a Mac without one, not a missing measure.
            if s.sensors.fans.is_empty() {
                return if s.sensors.temperatures.is_empty() { waiting() } else { vec!["Aucun ventilateur".into()] };
            }
            let mut lines: Vec<String> = s
                .sensors
                .fans
                .iter()
                .enumerate()
                .map(|(i, f)| match (f.min, f.max) {
                    (Some(min), Some(max)) => format!("Ventilateur {} : {} tr/min ({} à {})", i + 1, decimal(f.rpm as f64, 0), decimal(min as f64, 0), decimal(max as f64, 0)),
                    _ => format!("Ventilateur {} : {} tr/min", i + 1, decimal(f.rpm as f64, 0)),
                })
                .collect();
            lines.extend(thermal(s.sensors.thermal));
            lines
        }
        Widget::Battery => {
            let Some(b) = &s.sensors.battery else { return vec!["Batterie : aucune batterie".into()] };
            let state = match (b.charging, b.external) {
                (true, _) => "en charge",
                (false, true) => "sur secteur",
                (false, false) => "sur batterie",
            };
            let mut lines = vec![format!("Batterie : {}, {state}", percent(b.charge))];
            if let Some(m) = b.minutes_left {
                lines.push(format!("{} : {}", if b.charging { "Pleine dans" } else { "Autonomie" }, duration(m as u64 * 60)));
            }
            lines.push(format!("{} cycles{}", b.cycles, b.health.map_or(String::new(), |h| format!(" · santé {}", percent(h)))));
            lines
        }
        Widget::Swap => {
            let Some(m) = &s.memory else { return waiting() };
            let mut lines = vec![format!("Swap : {} sur {}", format_bytes(m.swap_used), format_bytes(m.swap_total))];
            lines.extend(pressure(m.pressure_free));
            lines
        }
    }
}

/// The memory pressure, as Activity Monitor names it.
fn pressure(free: Option<f32>) -> Option<String> {
    let f = free?;
    let state = if f < 0.15 {
        "forte"
    } else if f < 0.30 {
        "moyenne"
    } else {
        "faible"
    };
    Some(format!("Pression mémoire : {state} ({} libre)", percent(f)))
}

fn thermal(t: Option<Thermal>) -> Option<String> {
    Some(match t? {
        Thermal::Nominal => "État thermique : normal".to_string(),
        Thermal::Throttled(limit) => format!("État thermique : bridé à {limit} %"),
    })
}

/// The `TOP` heaviest processes for a column, one line each.
fn heaviest(s: &Snapshot, by: SortBy) -> Vec<String> {
    let Some(p) = &s.processes else { return Vec::new() };
    p.top(by)
        .into_iter()
        .take(TOP)
        .map(|p| match by {
            SortBy::Memory => format!("• {} : {}", p.name, format_bytes(p.memory)),
            _ => format!("• {} : {} %", p.name, decimal(p.cpu as f64 * 100.0, 0)),
        })
        .collect()
}

/// The biggest folder in `~/Library/Caches`, with its size.
pub fn largest_cache(env: &Env) -> Option<(String, u64)> {
    std::fs::read_dir(env.caches())
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), actions::folder_size(&e.path()).0))
        .max_by_key(|c| c.1)
}

/// A quick action from the menu: it runs without a preview, so only a reversible one is allowed.
pub fn quick(env: &Env, action: Action) -> Result<Prepared, String> {
    let p = actions::prepare(env, action)?;
    if p.level != Level::Reversible {
        return Err("Seules les actions réversibles sont proposées dans la barre des menus.".into());
    }
    Ok(p)
}

struct Icon {
    widget: Widget,
    tray: TrayIcon,
    /// The glyph on screen, to change it only when it differs (the battery, once it charges).
    glyph: &'static [u8],
    menu: Menu,
    /// The detail lines at the top of the menu, as many as the last reading had.
    lines: Vec<MenuItem>,
    alerts: MenuItem,
    clear: MenuItem,
    undo: MenuItem,
}

struct Bar {
    ctx: egui::Context,
    env: Option<Env>,
    icons: Vec<Icon>,
    last: Box<Snapshot>,
    largest: Option<(String, u64)>,
    /// The last alert of the surveillance rules, if any.
    alert: Option<String>,
    /// The journal entry of the last quick action, to undo it.
    undo: Option<u64>,
    note: Option<String>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Sets up the menu bar from the main thread, once, and shows the chosen widgets.
pub fn install(ctx: egui::Context, env: Option<Env>, settings: &Settings) {
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        let id = e.id.0.clone();
        DispatchQueue::main().exec_async(move || handle(&id));
    }));
    BAR.with(|b| {
        *b.borrow_mut() = Some(Bar { ctx, env, icons: Vec::new(), last: Box::default(), largest: None, alert: None, undo: None, note: None });
    });
    rebuild(settings);
}

/// Shows the widgets of `settings`, in order. Call it from the main thread.
pub fn rebuild(settings: &Settings) {
    BAR.with(|b| {
        let mut b = b.borrow_mut();
        let Some(bar) = b.as_mut() else { return };
        bar.icons.clear();
        // macOS puts the newest status item on the left: create them right to left.
        for &w in settings.widgets.iter().rev() {
            match icon(w, bar) {
                Ok(i) => bar.icons.push(i),
                Err(e) => eprintln!("menu bar widget {}: {e}", w.name()),
            }
        }
        refresh(bar);
    });
}

/// Hands a reading to the widgets, from any thread.
pub fn post(s: Snapshot) {
    let s = Box::new(s);
    DispatchQueue::main().exec_async(move || {
        with_bar(|bar| {
            bar.last = s;
            // The alerts come from `mac-anal check`, a separate process: read them with each reading.
            bar.alert = bar.env.as_ref().and_then(|env| crate::rules::alerts(env).pop()).map(|a| a.message);
            refresh(bar);
        })
    });
}

/// Hands the biggest cache to the menus, from any thread.
pub fn post_largest(largest: Option<(String, u64)>) {
    DispatchQueue::main().exec_async(move || {
        with_bar(|bar| {
            bar.largest = largest;
            refresh(bar);
        })
    });
}

/// Whether any widget is shown, so closing the window hides it instead of quitting.
pub fn active() -> bool {
    BAR.with(|b| b.borrow().as_ref().is_some_and(|bar| !bar.icons.is_empty()))
}

fn with_bar(f: impl FnOnce(&mut Bar)) {
    BAR.with(|b| {
        if let Some(bar) = b.borrow_mut().as_mut() {
            f(bar);
        }
    });
}

fn icon(w: Widget, bar: &Bar) -> Result<Icon, String> {
    let alerts = MenuItem::with_id(ALERT, "Aucune alerte", false, None);
    let clear = MenuItem::with_id(CLEAR, "Vider le plus gros cache", false, None);
    let undo = MenuItem::with_id(UNDO, "Annuler la dernière action", false, None);
    let open = MenuItem::with_id(OPEN, "Ouvrir Mac Anal", true, None);
    let quit = MenuItem::with_id(QUIT, "Quitter Mac Anal", true, None);
    let menu = Menu::new();
    let sep = PredefinedMenuItem::separator;
    menu.append_items(&[&sep(), &alerts, &sep(), &clear, &undo, &sep(), &open, &quit]).map_err(|e| e.to_string())?;
    let glyph = glyph(w, &bar.last);
    let tray = TrayIconBuilder::new()
        .with_icon_templated(image(glyph)?)
        .with_title(title(w, &bar.last))
        .with_tooltip(format!("Mac Anal : {}", w.name()))
        .with_menu(Box::new(menu.clone()))
        .build()
        .map_err(|e| e.to_string())?;
    let mut icon = Icon { widget: w, tray, glyph, menu, lines: Vec::new(), alerts, clear, undo };
    set_lines(&mut icon, &detail(w, &bar.last));
    Ok(icon)
}

/// Puts `texts` at the top of the menu, adding or removing lines so there are exactly as many.
fn set_lines(icon: &mut Icon, texts: &[String]) {
    while icon.lines.len() > texts.len() {
        if let Some(item) = icon.lines.pop() {
            let _ = icon.menu.remove(&item);
        }
    }
    for (i, text) in texts.iter().enumerate() {
        match icon.lines.get(i) {
            Some(item) => item.set_text(text),
            None => {
                // Greyed out: a line of detail, not an action.
                let item = MenuItem::new(text, false, None);
                if icon.menu.insert(&item, i).is_ok() {
                    icon.lines.push(item);
                }
            }
        }
    }
}

fn refresh(bar: &mut Bar) {
    let clear_text = match &bar.largest {
        Some((name, bytes)) => format!("Vider le plus gros cache ({name}, {})", format_bytes(*bytes)),
        None => "Vider le plus gros cache".into(),
    };
    let undo_text = bar.note.clone().map_or("Annuler la dernière action".into(), |n| format!("Annuler : {n}"));
    let last = &bar.last;
    for i in &mut bar.icons {
        i.tray.set_title(Some(title(i.widget, last)));
        let glyph = glyph(i.widget, last);
        if !std::ptr::eq(glyph, i.glyph) {
            if let Ok(img) = image(glyph) {
                let _ = i.tray.set_icon_templated(Some(img));
                i.glyph = glyph;
            }
        }
        set_lines(i, &detail(i.widget, last));
        i.alerts.set_text(bar.alert.as_ref().map_or("Aucune alerte".to_string(), |a| format!("Expliquer l'alerte : {a}")));
        i.alerts.set_enabled(bar.alert.is_some());
        i.clear.set_text(&clear_text);
        i.clear.set_enabled(bar.largest.is_some() && bar.env.is_some());
        i.undo.set_text(&undo_text);
        i.undo.set_enabled(bar.undo.is_some());
    }
}

fn handle(id: &str) {
    with_bar(|bar| {
        match id {
            ALERT => {
                if let (Some(a), Ok(mut e)) = (bar.alert.clone(), EXPLAIN.lock()) {
                    *e = Some(a);
                }
                bar.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                bar.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                bar.ctx.request_repaint();
            }
            OPEN => {
                bar.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                bar.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                bar.ctx.request_repaint();
            }
            // Nothing is left unsaved: the settings are written as soon as they change.
            QUIT => std::process::exit(0),
            CLEAR => {
                let (Some(env), Some((name, _))) = (&bar.env, bar.largest.clone()) else { return };
                match quick(env, Action::ClearCache { name: name.clone() }).and_then(|p| journal::run(env, Origin::MenuBar, &p, false)) {
                    Ok(done) => {
                        bar.undo = done.entry.filter(|_| done.undo.is_some());
                        bar.note = Some(format!("vider {name}"));
                        bar.largest = None;
                    }
                    Err(e) => eprintln!("quick action: {e}"),
                }
            }
            UNDO => {
                if let (Some(env), Some(id)) = (&bar.env, bar.undo.take()) {
                    if let Err(e) = journal::undo(env, id) {
                        eprintln!("undo: {e}");
                    }
                }
                bar.note = None;
            }
            _ => {}
        }
        refresh(bar);
    });
}

/// Finds the biggest cache now and then, on its own thread: walking the caches is slow.
pub fn watch_largest(env: Env) {
    std::thread::Builder::new()
        .name("largest-cache".into())
        .spawn(move || loop {
            post_largest(largest_cache(&env));
            std::thread::sleep(std::time::Duration::from_secs(300));
        })
        .expect("start the cache thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview::{Overview, Usage};

    const GB: u64 = 1_000_000_000;

    #[test]
    fn titles_show_the_live_value_or_wait() {
        let mut s = Snapshot::sample();
        s.overview = Overview {
            disk: Some(Usage { used: 312 * GB, total: 494 * GB }),
            memory: Some(Usage { used: 11 * GB, total: 16 * GB }),
            cpu: Some(0.234),
        };
        assert_eq!(title(Widget::Disk, &s), "182 Go");
        assert_eq!(title(Widget::Memory, &s), "69 %");
        assert_eq!(title(Widget::Cpu, &s), "23 %");
        assert_eq!(title(Widget::Gpu, &s), "31 %");
        assert_eq!(title(Widget::Network, &s), "↓2,4 Mo/s ↑319 Ko/s");
        assert_eq!(title(Widget::Temperature, &s), "58 °C");
        assert_eq!(title(Widget::Fan, &s), "1210 tr/min");
        assert_eq!(title(Widget::Battery, &s), "82 %");
        assert_eq!(title(Widget::Swap, &s), "1,2 Go");
        let empty = Snapshot::default();
        for w in Widget::ALL {
            assert_eq!(title(w, &empty), "…", "{w:?}");
        }
    }

    /// The size of every glyph, in pixels.
    const GLYPH: (u32, u32) = (40, 44);
    /// The names of the glyph files, the battery's two states included.
    const GLYPHS: [&str; 10] = ["disk", "memory", "cpu", "gpu", "network", "temperature", "fan", "battery", "battery-charging", "swap"];

    #[test]
    fn every_widget_has_a_glyph_of_the_same_size() {
        let s = Snapshot::sample();
        let mut seen = Vec::new();
        for w in Widget::ALL {
            let png = glyph(w, &s);
            let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap().into_rgba8();
            // One box for all, so they line up; a template image is black, its shape in the alpha.
            assert_eq!(img.dimensions(), GLYPH, "{w:?}");
            assert!(img.pixels().all(|p| p.0[..3] == [0, 0, 0]), "{w:?}");
            let inked: Vec<u32> = img.enumerate_pixels().filter(|(_, _, p)| p.0[3] > 0).map(|(_, y, _)| y).collect();
            assert!(!inked.is_empty(), "{w:?}");
            // Drawn around the middle of the box: the glyphs share one centre line.
            let (top, bottom) = (*inked.iter().min().unwrap(), *inked.iter().max().unwrap());
            assert!((top + bottom).abs_diff(GLYPH.1) <= 4, "{w:?}: {top} to {bottom}");
            assert!(image(png).is_ok());
            assert!(!seen.contains(&png), "{w:?} shares a glyph");
            seen.push(png);
        }
    }

    #[test]
    fn a_charging_battery_changes_its_glyph() {
        let mut s = Snapshot::sample();
        let charging = glyph(Widget::Battery, &s);
        s.sensors.battery.as_mut().unwrap().charging = false;
        assert!(!std::ptr::eq(charging, glyph(Widget::Battery, &s)));
    }

    /// `cargo test -- --ignored menubar_glyphs_are_current` draws the glyphs again from their SVG.
    #[test]
    #[ignore]
    fn menubar_glyphs_are_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/menubar");
        for name in GLYPHS {
            let svg = std::fs::read(dir.join(format!("{name}.svg"))).unwrap();
            let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).unwrap();
            let mut pixmap = resvg::tiny_skia::Pixmap::new(GLYPH.0, GLYPH.1).unwrap();
            // The SVG is 20 × 22, the glyph twice that.
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(2.0, 2.0), &mut pixmap.as_mut());
            pixmap.save_png(dir.join(format!("{name}.png"))).unwrap();
        }
    }

    #[test]
    fn every_menu_starts_with_a_summary_then_its_detail() {
        let s = Snapshot::sample();
        assert_eq!(detail(Widget::Disk, &s)[0], "Disque : 182 Go libres sur 494 Go");
        assert!(detail(Widget::Disk, &s).iter().any(|l| l.starts_with("Lecture 18 Mo/s")));
        let cpu = detail(Widget::Cpu, &s);
        assert_eq!(cpu[1], "Charge moyenne 1, 5, 15 min : 3,42 · 2,87 · 2,51");
        assert_eq!(cpu[3], "• Xcode : 137 %");
        assert_eq!(cpu.len(), 3 + TOP);
        let mem = detail(Widget::Memory, &s);
        assert!(mem.contains(&"Pression mémoire : faible (44 % libre)".to_string()));
        assert_eq!(mem.last().unwrap(), "• kernel_task : 1,1 Go");
        assert_eq!(detail(Widget::Network, &s)[1], "en0 (192.168.1.24) : ↓ 2,4 Mo/s ↑ 310 Ko/s");
        assert_eq!(detail(Widget::Temperature, &s).last().unwrap(), "État thermique : normal");
        assert_eq!(detail(Widget::Fan, &s)[0], "Ventilateur 1 : 1210 tr/min (1000 à 4900)");
        assert_eq!(detail(Widget::Battery, &s)[0], "Batterie : 82 %, en charge");
        assert_eq!(detail(Widget::Gpu, &s), ["GPU : 31 % (Apple M4 Pro)"]);
    }

    #[test]
    fn a_menu_waits_for_its_measure_and_says_what_is_missing() {
        let empty = Snapshot::default();
        assert_eq!(detail(Widget::Memory, &empty), ["Mémoire : mesure en cours"]);
        assert_eq!(detail(Widget::Swap, &empty), ["Swap : mesure en cours"]);
        assert_eq!(detail(Widget::Battery, &empty), ["Batterie : aucune batterie"]);
        let mut no_fan = Snapshot::sample();
        no_fan.sensors.fans.clear();
        assert_eq!(detail(Widget::Fan, &no_fan), ["Aucun ventilateur"]);
    }

    #[test]
    fn the_largest_cache_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        assert_eq!(largest_cache(&env), None);
        for (name, size) in [("small", 10), ("big", 5000), ("mid", 300)] {
            std::fs::create_dir_all(env.caches().join(name)).unwrap();
            std::fs::write(env.caches().join(name).join("f"), vec![0u8; size]).unwrap();
        }
        assert_eq!(largest_cache(&env), Some(("big".into(), 5000)));
    }

    #[test]
    fn quick_actions_are_reversible_only() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf() };
        std::fs::create_dir_all(env.caches().join("big")).unwrap();
        assert_eq!(quick(&env, Action::ClearCache { name: "big".into() }).unwrap().level, Level::Reversible);
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert!(quick(&env, Action::QuitProcess { pid: child.id() }).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
