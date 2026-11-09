//! The brand drawn with egui shapes: the logotype and the safety icons.

use egui::epaint::{PathShape, TextShape};
use egui::text::{LayoutJob, TextFormat};
use egui::emath::Rot2;
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::theme::{self, CORAL, CREAM, INK};

/// The letters swept away, from the design document: offset (in F), rotation, opacity.
const FLYING: [(char, Vec2, f32, f32); 3] = [
    ('y', Vec2::new(0.0, 0.0), 10.0, 1.0),
    ('s', Vec2::new(0.088, -0.294), 30.0, 0.7),
    ('e', Vec2::new(0.206, -0.706), 60.0, 0.4),
];
/// Bricolage Grotesque's ascent and descent, and the logotype's line height, in F.
const ASCENT: f32 = 0.93;
const DESCENT: f32 = 0.27;
const LINE: f32 = 0.9;

/// « Mac Anal », the broom, then « y », « s », « e » flying off, at text size `f`.
pub fn logotype(ui: &mut egui::Ui, f: f32) -> egui::Response {
    let (ink, bristles) = if ui.visuals().dark_mode { (CREAM, CORAL) } else { (INK, CORAL) };
    let font = FontId::new(f, FontFamily::Name(theme::BRAND.into()));
    let galley = |text: &str, spacing: f32| {
        let mut job = LayoutJob::default();
        job.append(text, 0.0, TextFormat { font_id: font.clone(), color: ink, extra_letter_spacing: spacing * f, ..Default::default() });
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let name = galley("Mac Anal", -0.035);
    let letters: Vec<_> = FLYING.iter().map(|(c, ..)| galley(&c.to_string(), -0.02)).collect();
    let broom = Vec2::new(0.853, 1.235) * f;
    let width = name.size().x + (0.147 + 0.059) * f + broom.x + letters.iter().map(|g| g.size().x).sum::<f32>();
    // Room above for the letters that fly up, and the protection margin of 0.5 F around.
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width + 0.3 * f, 1.6 * f), Sense::hover());
    let painter = ui.painter_at(rect.expand(0.5 * f));
    // Every box sits on one bottom line; with a line height of 0.9 F, it lies 0.12 F under the baseline.
    let baseline = rect.bottom() - 0.2 * f;
    let box_bottom = baseline + DESCENT * f - (ASCENT + DESCENT - LINE) / 2.0 * f;
    let top_of = |g: &std::sync::Arc<egui::Galley>| baseline - g.rows[0].glyphs[0].pos.y - g.rows[0].pos.y;

    let mut x = rect.left();
    painter.add(TextShape::new(Pos2::new(x, top_of(&name)), name.clone(), ink));
    x += name.size().x + 0.147 * f;
    paint_broom(&painter, Rect::from_min_size(Pos2::new(x, box_bottom + 0.088 * f - broom.y), broom), ink, bristles);
    x += broom.x + 0.059 * f;
    for ((_, offset, degrees, opacity), g) in FLYING.iter().zip(letters) {
        let pos = Pos2::new(x, top_of(&g)) + *offset * f;
        // Turned around the centre of the letter's box, as CSS does.
        let centre = Pos2::new(pos.x + g.size().x / 2.0, box_bottom - LINE / 2.0 * f + offset.y * f);
        let angle = degrees.to_radians();
        let pos = centre + Rot2::from_angle(angle) * (pos - centre);
        x += g.size().x;
        painter.add(TextShape::new(pos, g, ink).with_angle(angle).with_opacity_factor(*opacity));
    }
    response
}

/// The broom of `broom.svg` (view box 54 × 78), drawn in `rect`.
fn paint_broom(painter: &egui::Painter, rect: Rect, ink: Color32, bristles: Color32) {
    let k = rect.width() / 54.0;
    let rot = Rot2::from_angle((-28.0f32).to_radians());
    let at = |x: f32, y: f32| rect.min + (Vec2::new(27.0, 49.0) + rot * Vec2::new(x, y)) * k;
    let line = |a: Pos2, b: Pos2, w: f32| {
        // Round caps, as `stroke-linecap="round"`.
        painter.line_segment([a, b], Stroke::new(w * k, ink));
        painter.circle_filled(a, w * k / 2.0, ink);
        painter.circle_filled(b, w * k / 2.0, ink);
    };
    line(at(0.0, -52.0), at(0.0, -9.0), 5.0);
    let mut head = vec![at(-7.0, -3.0), at(7.0, -3.0), at(16.0, 22.0)];
    // The quadratic curve Q0 26.5 -16 22, sampled.
    for i in 1..=12 {
        let t = i as f32 / 12.0;
        let (p0, p1, p2) = (Vec2::new(16.0, 22.0), Vec2::new(0.0, 26.5), Vec2::new(-16.0, 22.0));
        let p = p0 * (1.0 - t) * (1.0 - t) + p1 * 2.0 * t * (1.0 - t) + p2 * t * t;
        head.push(at(p.x, p.y));
    }
    painter.add(PathShape::convex_polygon(head, bristles, Stroke::new(2.6 * k, ink)));
    for (a, b) in [((-3.5, 1.0), (-7.5, 21.5)), ((0.0, 1.0), (0.0, 23.0)), ((3.5, 1.0), (7.5, 21.5))] {
        line(at(a.0, a.1), at(b.0, b.1), 1.6);
    }
    // The ferrule: a 17 × 8 rectangle with 2.5 corners.
    let mut ferrule = Vec::new();
    for (cx, cy, start) in [(6.0, -7.5, -90.0), (6.0, -4.5, 0.0), (-6.0, -4.5, 90.0), (-6.0, -7.5, 180.0)] {
        for i in 0..=4 {
            let a = (start + 22.5 * i as f32).to_radians();
            ferrule.push(at(cx + 2.5 * a.cos(), cy + 2.5 * a.sin()));
        }
    }
    painter.add(PathShape::convex_polygon(ferrule, ink, Stroke::NONE));
}

/// How safe an action or a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    Safe,
    Review,
    Risky,
}

impl Safety {
    pub fn color(self) -> Color32 {
        match self {
            Safety::Safe => theme::SAFE,
            Safety::Review => theme::REVIEW,
            Safety::Risky => theme::RISKY,
        }
    }
}

/// The coloured icon of a safety level; its label is drawn beside it, in the text colour.
pub fn safety_icon(ui: &mut egui::Ui, safety: Safety) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let color = safety.color();
    match safety {
        // A check in a disc.
        Safety::Safe => {
            p.circle_filled(c, 7.0, color);
            p.add(Shape::line(vec![c + Vec2::new(-3.2, 0.2), c + Vec2::new(-0.8, 2.6), c + Vec2::new(3.4, -2.2)], Stroke::new(1.8, Color32::WHITE)));
        }
        // An eye: an iris in a disc.
        Safety::Review => {
            p.circle_filled(c, 7.0, color);
            p.circle_stroke(c, 3.0, Stroke::new(1.4, INK));
            p.circle_filled(c, 1.3, INK);
        }
        // An exclamation mark in a triangle.
        Safety::Risky => {
            let r = rect.shrink(0.5);
            p.add(Shape::convex_polygon(vec![r.center_top(), r.right_bottom(), r.left_bottom()], color, Stroke::NONE));
            p.line_segment([c + Vec2::new(0.0, -2.0), c + Vec2::new(0.0, 2.2)], Stroke::new(1.6, Color32::WHITE));
            p.circle_filled(c + Vec2::new(0.0, 4.4), 0.9, Color32::WHITE);
        }
    }
    response
}
