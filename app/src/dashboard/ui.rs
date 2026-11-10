//! The dashboard on screen: a grid of cards that follows the window's width, the processes below.

use egui::{Align2, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};

use super::hardware::Backup;
use super::history::Point;
use super::procs::TOP;
use super::format::{decimal, duration, percent, rate, sensor_name};
use super::{Dashboard, Series, SortBy, Thermal};
use crate::overview::format_bytes;
use crate::theme;

/// Narrowest a card column gets before the grid drops a column.
const COLUMN: f32 = 320.0;
const CURVE_HEIGHT: f32 = 38.0;
/// Temperatures listed before the rest is folded.
const TEMPERATURES: usize = 6;

type Card = fn(&Dashboard, &mut Ui);

pub fn show(d: &mut Dashboard, ui: &mut Ui) {
    ui.add_space(24.0);
    ui.heading("Tableau de bord");
    ui.add_space(4.0);
    identity(d, ui);
    ui.add_space(12.0);

    let mut cards: Vec<(&str, Card)> = vec![("Processeur", cpu), ("Mémoire", memory), ("Graphisme", gpu), ("Réseau", network), ("Disques", disks), ("Capteurs", sensors)];
    // A Mac without a battery has no battery card at all.
    if d.snapshot.sensors.battery.is_some() {
        cards.push(("Batterie", battery));
    }
    cards.push(("Matériel connecté", hardware));

    let columns = ((ui.available_width() / COLUMN).floor() as usize).clamp(1, 3);
    let dash: &Dashboard = d;
    ui.columns(columns, |cols| {
        for (i, (title, body)) in cards.iter().enumerate() {
            card(&mut cols[i % columns], title, |ui| body(dash, ui));
            cols[i % columns].add_space(12.0);
        }
    });
}

/// The processes, a section of their own.
pub fn show_processes(d: &mut Dashboard, ui: &mut Ui) {
    ui.add_space(24.0);
    ui.heading("Processus");
    ui.add_space(12.0);
    card(ui, "Les plus gourmands", |ui| processes(d, ui));
}

/// One section: its title, then its body on a surface.
fn card(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    theme::card(ui, |ui| {
        ui.label(RichText::new(title).text_style(theme::title()));
        ui.add_space(6.0);
        body(ui);
    });
}

fn unavailable(ui: &mut Ui) {
    ui.label(RichText::new("indisponible").weak());
}

/// A big figure, then a smaller caption beside it.
fn figure(ui: &mut Ui, value: &str, caption: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(value).font(theme::figure_font()));
        ui.label(RichText::new(caption).weak());
    });
}

fn row(ui: &mut Ui, label: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(value.into()));
    });
}

fn bar(ui: &mut Ui, fraction: f32) {
    let p = theme::palette(ui);
    ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).fill(p.data).desired_height(8.0).corner_radius(CornerRadius::same(4)));
}

fn identity(d: &Dashboard, ui: &mut Ui) {
    match &d.snapshot.identity {
        Some(id) => {
            let line = format!("{} ({}) · {} · {} · Darwin {} · {} · allumé depuis {}", id.model, id.model_id, id.chip, id.os, id.kernel, id.host, duration(id.uptime));
            ui.label(RichText::new(line).weak());
        }
        None => {
            ui.label(RichText::new("Lecture du Mac…").weak());
        }
    }
}

fn cpu(d: &Dashboard, ui: &mut Ui) {
    let Some(c) = &d.snapshot.cpu else { return unavailable(ui) };
    let total = c.cores.iter().sum::<f32>() / c.cores.len().max(1) as f32;
    figure(ui, &percent(total), &format!("{} cœurs performance, {} efficacité", c.performance, c.efficiency));
    curve(ui, &d.history.points(Series::Cpu), Some(1.0), |v| percent(v as f32));
    ui.add_space(6.0);
    cores(ui, c);
    ui.add_space(6.0);
    row(ui, "Charge moyenne 1, 5, 15 min", c.load.map(|l| decimal(l, 2)).join(" · "));
    if c.frequency_mhz > 0 {
        row(ui, "Fréquence", format!("{} GHz", decimal(c.frequency_mhz as f64 / 1000.0, 2)));
    }
}

/// One small column per core, efficiency cores in Corail.
fn cores(ui: &mut Ui, c: &super::Cpu) {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
    let n = c.cores.len().max(1) as f32;
    let gap = 3.0;
    let w = ((rect.width() - gap * (n - 1.0)) / n).max(1.0);
    let painter = ui.painter_at(rect);
    let mut hovered = None;
    for (i, load) in c.cores.iter().enumerate() {
        let x = rect.left() + i as f32 * (w + gap);
        let track = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, rect.height()));
        painter.rect_filled(track, CornerRadius::same(2), p.free);
        let h = rect.height() * load;
        let fill = if c.is_efficiency(i) { theme::CORAL } else { p.data };
        painter.rect_filled(Rect::from_min_max(Pos2::new(x, rect.bottom() - h), Pos2::new(x + w, rect.bottom())), CornerRadius::same(2), fill);
        if response.hover_pos().is_some_and(|pos| track.x_range().contains(pos.x)) {
            hovered = Some(i);
        }
    }
    if let Some(i) = hovered {
        let kind = if c.is_efficiency(i) { "efficacité" } else { "performance" };
        response.on_hover_text_at_pointer(format!("Cœur {} ({kind}) : {}", i + 1, percent(c.cores[i])));
    }
}

fn memory(d: &Dashboard, ui: &mut Ui) {
    let Some(m) = &d.snapshot.memory else { return unavailable(ui) };
    figure(ui, &format_bytes(m.used()), &format!("utilisés sur {}", format_bytes(m.total)));
    curve(ui, &d.history.points(Series::Memory), Some(1.0), |v| percent(v as f32));
    ui.add_space(6.0);
    let p = theme::palette(ui);
    let parts = [
        ("Applications", m.app, p.data),
        ("Câblée", m.wired, theme::CORAL),
        ("Compressée", m.compressed, theme::YELLOW),
        ("Cache", m.cached, p.data_quiet),
    ];
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 12.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, p.free);
    let mut x = rect.left();
    for (_, bytes, color) in parts {
        let w = rect.width() * bytes as f32 / m.total.max(1) as f32;
        painter.rect_filled(Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, rect.height())), CornerRadius::ZERO, color);
        x += w;
        // A divider, so a pale share does not melt into the free track.
        painter.vline(x, rect.y_range(), Stroke::new(1.0, p.line));
    }
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.5, p.line), egui::StrokeKind::Inside);
    ui.add_space(4.0);
    for (name, bytes, color) in parts {
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle(dot.center(), 4.5, color, Stroke::new(1.0, p.line));
            ui.label(RichText::new(name).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(format_bytes(bytes)));
        });
    }
    row(ui, "Libre", format_bytes(m.free));
    row(ui, "Swap", format!("{} / {}", format_bytes(m.swap_used), format_bytes(m.swap_total)));
    curve(ui, &d.history.points(Series::Swap), None, |v| format_bytes(v as u64));
    if let Some(free) = m.pressure_free {
        let state = match free {
            f if f < 0.15 => "forte",
            f if f < 0.30 => "moyenne",
            _ => "faible",
        };
        row(ui, "Pression mémoire", format!("{state} ({} libre)", percent(free)));
    }
}

fn gpu(d: &Dashboard, ui: &mut Ui) {
    let gpus = &d.snapshot.gpus;
    if gpus.is_empty() {
        return unavailable(ui);
    }
    for g in gpus {
        match g.usage {
            Some(u) => figure(ui, &percent(u), &g.name),
            None => row(ui, &g.name, "utilisation indisponible"),
        }
    }
    curve(ui, &d.history.points(Series::Gpu), Some(1.0), |v| percent(v as f32));
}

fn network(d: &Dashboard, ui: &mut Ui) {
    let s = &d.snapshot;
    if s.interfaces.is_empty() {
        return unavailable(ui);
    }
    let (down, up) = s.network_rate();
    figure(ui, &format!("↓ {}", rate(down)), &format!("↑ {}", rate(up)));
    theme::eyebrow(ui, "Reçu");
    curve(ui, &d.history.points(Series::NetIn), None, |v| rate(v as u64));
    theme::eyebrow(ui, "Envoyé");
    curve(ui, &d.history.points(Series::NetOut), None, |v| rate(v as u64));
    ui.add_space(6.0);
    for i in &s.interfaces {
        let name = match &i.address {
            Some(a) => format!("{} ({a})", i.name),
            None => i.name.clone(),
        };
        row(ui, &name, format!("↓ {} ↑ {}", rate(i.received_per_s), rate(i.sent_per_s)));
        ui.label(RichText::new(format!("depuis le démarrage : {} reçus, {} envoyés", format_bytes(i.received_total), format_bytes(i.sent_total))).weak().small());
    }
}

fn disks(d: &Dashboard, ui: &mut Ui) {
    let s = &d.snapshot;
    if s.volumes.is_empty() {
        return unavailable(ui);
    }
    let (read, written) = s.disk_rate();
    figure(ui, &format!("Lecture {}", rate(read)), &format!("Écriture {}", rate(written)));
    theme::eyebrow(ui, "Lecture");
    curve(ui, &d.history.points(Series::DiskRead), None, |v| rate(v as u64));
    theme::eyebrow(ui, "Écriture");
    curve(ui, &d.history.points(Series::DiskWrite), None, |v| rate(v as u64));
    ui.add_space(6.0);
    for v in &s.volumes {
        let kind = if v.removable { ", amovible" } else { "" };
        row(ui, &format!("{} ({}{kind})", v.name, v.file_system), format!("{} / {}", format_bytes(v.used), format_bytes(v.total)));
        bar(ui, v.used as f32 / v.total.max(1) as f32);
        ui.label(RichText::new(v.mount.as_str()).weak().small());
    }
}

fn sensors(d: &Dashboard, ui: &mut Ui) {
    let s = &d.snapshot.sensors;
    match s.temperatures.first() {
        Some((name, t)) => figure(ui, &format!("{} °C", decimal(*t as f64, 0)), &format!("au plus chaud ({})", sensor_name(name))),
        None => {
            ui.label(RichText::new("Températures indisponibles").weak());
        }
    }
    curve(ui, &d.history.points(Series::Temperature), None, |v| format!("{} °C", decimal(v, 1)));
    for (name, t) in s.temperatures.iter().take(TEMPERATURES) {
        row(ui, &sensor_name(name), format!("{} °C", decimal(*t as f64, 1)));
    }
    if s.temperatures.len() > TEMPERATURES {
        ui.collapsing(format!("{} autres capteurs", s.temperatures.len() - TEMPERATURES), |ui| {
            for (name, t) in &s.temperatures[TEMPERATURES..] {
                row(ui, &sensor_name(name), format!("{} °C", decimal(*t as f64, 1)));
            }
        });
    }
    ui.add_space(6.0);
    if s.fans.is_empty() {
        row(ui, "Ventilateurs", "aucun ventilateur");
    }
    for (i, f) in s.fans.iter().enumerate() {
        row(ui, &format!("Ventilateur {}", i + 1), format!("{:.0} tr/min", f.rpm));
        if let (Some(min), Some(max)) = (f.min, f.max) {
            if max > min {
                bar(ui, (f.rpm - min) / (max - min));
            }
        }
    }
    let thermal = match s.thermal {
        Some(Thermal::Nominal) => "normal".to_string(),
        Some(Thermal::Throttled(limit)) => format!("bridé à {limit} %"),
        None => "inconnu".to_string(),
    };
    row(ui, "État thermique", thermal);
}

fn battery(d: &Dashboard, ui: &mut Ui) {
    let Some(b) = &d.snapshot.sensors.battery else { return unavailable(ui) };
    let state = match (b.charging, b.external) {
        (true, _) => "en charge",
        (false, true) => "sur secteur",
        (false, false) => "sur batterie",
    };
    figure(ui, &percent(b.charge), state);
    bar(ui, b.charge);
    if let Some(m) = b.minutes_left {
        row(ui, if b.charging { "Pleine dans" } else { "Autonomie" }, duration(m as u64 * 60));
    }
    row(ui, "Cycles", b.cycles.to_string());
    if let Some(h) = b.health {
        row(ui, "Santé", percent(h));
    }
    if let Some(t) = b.temperature {
        row(ui, "Température", format!("{} °C", decimal(t as f64, 1)));
    }
}

fn hardware(d: &Dashboard, ui: &mut Ui) {
    let Some(h) = &d.hardware else {
        ui.label(RichText::new("Lecture en cours…").weak());
        return;
    };
    let backup = match &h.backup {
        Backup::Last(date) => date.clone(),
        Backup::Never => "aucune sauvegarde".to_string(),
        Backup::Unavailable(why) => why.clone(),
        Backup::Unknown => "inconnue".to_string(),
    };
    row(ui, "Dernière sauvegarde Time Machine", backup);
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Écrans ({})", h.displays.len())).strong());
    for s in &h.displays {
        let main = if s.main { ", principal" } else { "" };
        row(ui, &format!("{}{main}", s.name), s.resolution.clone().unwrap_or_default());
    }
    ui.add_space(6.0);
    ui.label(RichText::new(format!("USB ({})", h.usb.len())).strong());
    if h.usb.is_empty() {
        ui.label(RichText::new("aucun appareil").weak());
    }
    for name in &h.usb {
        ui.label(name.as_str());
    }
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Bluetooth ({})", h.bluetooth.len())).strong());
    if h.bluetooth.is_empty() {
        ui.label(RichText::new("aucun appareil connecté").weak());
    }
    for (name, kind) in &h.bluetooth {
        row(ui, name, kind.clone().unwrap_or_default());
    }
}

fn processes(d: &mut Dashboard, ui: &mut Ui) {
    let Some(p) = &d.snapshot.processes else { return unavailable(ui) };
    let mut counts = vec![format!("{} processus", p.count)];
    if let Some(t) = p.threads {
        counts.push(format!("{t} fils d'exécution"));
    }
    if let Some(f) = p.open_files {
        counts.push(format!("{f} fichiers ouverts"));
    }
    ui.label(RichText::new(counts.join(" · ")).weak());
    ui.add_space(6.0);
    let mut sort = d.sort;
    // A narrow window (the assistant open) scrolls the table sideways instead of cutting it.
    egui::ScrollArea::horizontal().show(ui, |ui| egui::Grid::new("processes").num_columns(5).striped(true).spacing([24.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new("Processus").strong());
        ui.label(RichText::new("PID").strong());
        for (label, by) in [("Processeur", SortBy::Cpu), ("Mémoire", SortBy::Memory), ("Disque", SortBy::Disk)] {
            let text = if sort == by { format!("{label} ▼") } else { label.to_string() };
            if ui.selectable_label(sort == by, text).on_hover_text("Trier par cette colonne").clicked() {
                sort = by;
            }
        }
        ui.end_row();
        for proc in p.top(d.sort) {
            ui.label(proc.name.as_str());
            ui.label(RichText::new(proc.pid.to_string()).weak());
            ui.label(format!("{} %", decimal(proc.cpu as f64 * 100.0, 1)));
            ui.label(format_bytes(proc.memory));
            ui.label(rate(proc.disk_per_s));
            ui.end_row();
        }
    }));
    if p.all.len() > TOP {
        ui.label(RichText::new(format!("Les {TOP} premiers sur {}.", p.all.len())).weak().small());
    }
    d.sort = sort;
}

/// A curve of the last ten minutes. `max` fixes the top of the scale (1 for a share); without it
/// the scale follows the highest point. Hovering shows the value and the time of the nearest point.
fn curve(ui: &mut Ui, points: &[Point], max: Option<f64>, format: impl Fn(f64) -> String) {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), CURVE_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), p.free);
    if points.len() < 2 {
        painter.text(rect.center(), Align2::CENTER_CENTER, "historique en cours", FontId::proportional(11.0), p.text_secondary);
        return;
    }
    let top = max.unwrap_or_else(|| points.iter().map(|(_, v)| *v).fold(0.0, f64::max)).max(f64::EPSILON);
    let last = points[points.len() - 1].0;
    let span = super::history::SPAN as f32;
    // The right edge is now; the curve fills from the left as the minutes pass.
    let x = |at: u64| rect.right() - last.saturating_sub(at) as f32 / span * rect.width();
    let y = |v: f64| rect.bottom() - 2.0 - (v / top).clamp(0.0, 1.0) as f32 * (rect.height() - 4.0);
    let line: Vec<Pos2> = points.iter().map(|(at, v)| Pos2::new(x(*at), y(*v))).collect();
    painter.add(Shape::line(line, Stroke::new(2.0, p.data)));
    if let Some(pos) = response.hover_pos() {
        if let Some((at, v)) = points.iter().min_by(|a, b| (x(a.0) - pos.x).abs().total_cmp(&(x(b.0) - pos.x).abs())) {
            let px = x(*at);
            painter.line_segment([Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())], Stroke::new(1.0, p.text_secondary));
            painter.circle(Pos2::new(px, y(*v)), 4.0, theme::CORAL, Stroke::new(1.5, p.line));
            response.on_hover_text_at_pointer(format!("{} à {}", format(*v), clock(*at)));
        }
    }
}

/// The local time of a moment, « 14:32:05 ».
fn clock(at: u64) -> String {
    // SAFETY: `localtime_r` writes only into the `tm` it is given.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = at as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        tm
    };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_shown_as_a_clock() {
        assert_eq!(clock(0).len(), 8);
    }
}
