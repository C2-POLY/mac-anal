//! The widgets of the macOS menu bar: one `NSStatusItem` per chosen widget, showing a live value
//! and holding a menu with the detail, the alerts and quick reversible actions.
//!
//! Status items only live on the main thread, so they are kept in a thread-local there, and every
//! update reaches them through the main GCD queue. That keeps them fresh while the window is hidden,
//! without going through egui.

use std::cell::RefCell;

use dispatch2::DispatchQueue;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::actions::{self, Action, Env, Level, Prepared, Undo};
use crate::overview::{format_bytes, Overview};
use crate::settings::{Settings, Widget};

const OPEN: &str = "open";
const QUIT: &str = "quit";
const CLEAR: &str = "clear-largest";
const UNDO: &str = "undo";

/// The text a widget shows in the menu bar.
pub fn title(w: Widget, o: &Overview) -> String {
    let pct = |f: f32| format!("{:.0} %", f * 100.0);
    match w {
        Widget::Disk => format!("💾 {}", o.disk.map(|d| format_bytes(d.total.saturating_sub(d.used))).unwrap_or_else(|| "…".into())),
        Widget::Memory => format!("🧠 {}", o.memory.map(|m| pct(m.used as f32 / m.total.max(1) as f32)).unwrap_or_else(|| "…".into())),
        Widget::Cpu => format!("⚡ {}", o.cpu.map(pct).unwrap_or_else(|| "…".into())),
    }
}

/// The first line of a widget's menu.
pub fn detail(w: Widget, o: &Overview) -> String {
    match w {
        Widget::Disk => o.disk.map_or("Disque : mesure en cours".into(), |d| {
            format!("Disque : {} libres sur {}", format_bytes(d.total.saturating_sub(d.used)), format_bytes(d.total))
        }),
        Widget::Memory => o.memory.map_or("Mémoire : mesure en cours".into(), |m| {
            format!("Mémoire : {} utilisés sur {}", format_bytes(m.used), format_bytes(m.total))
        }),
        Widget::Cpu => o.cpu.map_or("Processeur : mesure en cours".into(), |c| format!("Processeur : {:.0} % de charge", c * 100.0)),
    }
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
    detail: MenuItem,
    clear: MenuItem,
    undo: MenuItem,
}

struct Bar {
    ctx: egui::Context,
    env: Option<Env>,
    icons: Vec<Icon>,
    last: Overview,
    largest: Option<(String, u64)>,
    undo: Option<Undo>,
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
        *b.borrow_mut() = Some(Bar { ctx, env, icons: Vec::new(), last: Overview::default(), largest: None, undo: None, note: None });
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
pub fn post(o: Overview) {
    DispatchQueue::main().exec_async(move || {
        with_bar(|bar| {
            bar.last = o;
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
    let detail = MenuItem::new(detail(w, &bar.last), false, None);
    let alerts = MenuItem::new("Aucune alerte", false, None);
    let clear = MenuItem::with_id(CLEAR, "Vider le plus gros cache", false, None);
    let undo = MenuItem::with_id(UNDO, "Annuler la dernière action", false, None);
    let open = MenuItem::with_id(OPEN, "Ouvrir mac-anal", true, None);
    let quit = MenuItem::with_id(QUIT, "Quitter mac-anal", true, None);
    let menu = Menu::new();
    let sep = PredefinedMenuItem::separator;
    menu.append_items(&[&detail, &sep(), &alerts, &sep(), &clear, &undo, &sep(), &open, &quit]).map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_title(title(w, &bar.last))
        .with_tooltip(format!("mac-anal : {}", w.name()))
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Icon { widget: w, tray, detail, clear, undo })
}

fn refresh(bar: &Bar) {
    let clear_text = match &bar.largest {
        Some((name, bytes)) => format!("Vider le plus gros cache ({name}, {})", format_bytes(*bytes)),
        None => "Vider le plus gros cache".into(),
    };
    let undo_text = bar.note.clone().map_or("Annuler la dernière action".into(), |n| format!("Annuler : {n}"));
    for i in &bar.icons {
        i.tray.set_title(Some(title(i.widget, &bar.last)));
        i.detail.set_text(detail(i.widget, &bar.last));
        i.clear.set_text(&clear_text);
        i.clear.set_enabled(bar.largest.is_some() && bar.env.is_some());
        i.undo.set_text(&undo_text);
        i.undo.set_enabled(bar.undo.is_some());
    }
}

fn handle(id: &str) {
    with_bar(|bar| {
        match id {
            OPEN => {
                bar.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                bar.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                bar.ctx.request_repaint();
            }
            // Nothing is left unsaved: the settings are written as soon as they change.
            QUIT => std::process::exit(0),
            CLEAR => {
                let (Some(env), Some((name, _))) = (&bar.env, bar.largest.clone()) else { return };
                match quick(env, Action::ClearCache { name: name.clone() }).and_then(|p| actions::execute(env, &p, false)) {
                    Ok(done) => {
                        bar.undo = done.undo;
                        bar.note = Some(format!("vider {name}"));
                        bar.largest = None;
                    }
                    Err(e) => eprintln!("quick action: {e}"),
                }
            }
            UNDO => {
                if let Some(u) = bar.undo.take() {
                    if let Err(e) = actions::undo(&u) {
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
    use crate::overview::Usage;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn titles_show_the_live_value_or_wait() {
        let o = Overview {
            disk: Some(Usage { used: 312 * GB, total: 494 * GB }),
            memory: Some(Usage { used: 11 * GB, total: 16 * GB }),
            cpu: Some(0.234),
        };
        assert_eq!(title(Widget::Disk, &o), "💾 182 Go");
        assert_eq!(title(Widget::Memory, &o), "🧠 69 %");
        assert_eq!(title(Widget::Cpu, &o), "⚡ 23 %");
        assert_eq!(detail(Widget::Disk, &o), "Disque : 182 Go libres sur 494 Go");
        let empty = Overview::default();
        assert_eq!(title(Widget::Cpu, &empty), "⚡ …");
        assert_eq!(detail(Widget::Memory, &empty), "Mémoire : mesure en cours");
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
