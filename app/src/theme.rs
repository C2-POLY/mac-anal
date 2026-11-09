//! The tokens of the design document (« Mac Anal — Document de design »), applied to egui.

use std::sync::Arc;

use egui::epaint::text::{FontData, FontDefinitions, FontFamily, FontTweak, VariationCoords};
use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, Theme, Vec2, Visuals};

pub const YELLOW: Color32 = Color32::from_rgb(0xFF, 0xD4, 0x3B);
pub const INK: Color32 = Color32::from_rgb(0x16, 0x16, 0x16);
pub const CORAL: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x36);
pub const CREAM: Color32 = Color32::from_rgb(0xFF, 0xF7, 0xD6);

/// Safety colours: they tint an icon, never a text.
pub const SAFE: Color32 = Color32::from_rgb(0x0C, 0xA3, 0x0C);
pub const REVIEW: Color32 = Color32::from_rgb(0xFA, 0xB2, 0x19);
pub const RISKY: Color32 = Color32::from_rgb(0xD0, 0x3B, 0x3B);

/// The interface tokens of one appearance.
pub struct Palette {
    /// macOS `windowBackgroundColor`, never tinted.
    pub window: Color32,
    pub surface: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub accent: Color32,
    pub separator: Color32,
    /// First colour of the data palette, for the measure bars.
    pub data: Color32,
    /// « Espace libre » of the data palette: the track behind a bar.
    pub free: Color32,
    /// A push button at rest, then hovered, as macOS draws them.
    pub control: Color32,
    pub control_hover: Color32,
}

pub const LIGHT: Palette = Palette {
    window: Color32::from_rgb(0xEC, 0xEC, 0xEC),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    text: Color32::from_rgb(0x16, 0x16, 0x16),
    text_secondary: Color32::from_rgb(0x5E, 0x5C, 0x55),
    accent: Color32::from_rgb(0xC2, 0x3A, 0x12),
    separator: Color32::from_rgb(0xD9, 0xD9, 0xD9),
    data: Color32::from_rgb(0x2A, 0x78, 0xD6),
    free: Color32::from_rgb(0xE1, 0xE0, 0xD9),
    control: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    control_hover: Color32::from_rgb(0xF4, 0xF4, 0xF4),
};

pub const DARK: Palette = Palette {
    window: Color32::from_rgb(0x32, 0x32, 0x32),
    surface: Color32::from_rgb(0x1E, 0x1E, 0x1E),
    text: Color32::from_rgb(0xF3, 0xF1, 0xEA),
    text_secondary: Color32::from_rgb(0xB9, 0xB6, 0xAC),
    accent: Color32::from_rgb(0xF0, 0x64, 0x3C),
    separator: Color32::from_rgb(0x48, 0x48, 0x48),
    data: Color32::from_rgb(0x39, 0x87, 0xE5),
    free: Color32::from_rgb(0x2C, 0x2C, 0x2A),
    control: Color32::from_rgb(0x55, 0x55, 0x55),
    control_hover: Color32::from_rgb(0x62, 0x62, 0x62),
};

pub fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

/// Bricolage Grotesque, a variable font (SIL Open Font License, see `assets/OFL.txt`).
const BRICOLAGE: &[u8] = include_bytes!("../assets/BricolageGrotesque.ttf");
/// The macOS interface font, read where the system keeps it: never shipped with the app.
const SF: &str = "/System/Library/Fonts/SFNS.ttf";
const SF_MONO: &str = "/System/Library/Fonts/SFNSMono.ttf";

/// Brand family: the logotype and big figures only, never below 20 pt.
pub const BRAND: &str = "brand";
/// SF Semibold, the system `.headline`.
pub const HEADLINE: &str = "headline";

pub fn headline() -> TextStyle {
    TextStyle::Name(HEADLINE.into())
}

pub fn apply(ctx: &egui::Context) {
    let mut f = fonts(std::fs::read(SF).ok());
    if let Ok(mono) = std::fs::read(SF_MONO) {
        f.font_data.insert("sf-mono".into(), Arc::new(FontData::from_owned(mono)));
        f.families.entry(FontFamily::Monospace).or_default().insert(0, "sf-mono".into());
    }
    ctx.set_fonts(f);
    ctx.style_mut_of(Theme::Light, |s| style(s, &LIGHT));
    ctx.style_mut_of(Theme::Dark, |s| style(s, &DARK));
}

fn weight(data: FontData, wght: f32, opsz: Option<f32>) -> Arc<FontData> {
    let mut coords = VariationCoords::new([(b"wght", wght)]);
    if let Some(o) = opsz {
        coords.push(b"opsz", o);
    }
    Arc::new(data.tweak(FontTweak { coords, ..Default::default() }))
}

/// SF when the system has it, otherwise egui's own font.
pub fn fonts(sf: Option<Vec<u8>>) -> FontDefinitions {
    let mut f = FontDefinitions::default();
    let fallback = f.families[&FontFamily::Proportional].clone();
    f.font_data.insert("bricolage".into(), weight(FontData::from_static(BRICOLAGE), 800.0, Some(28.0)));
    f.families.insert(FontFamily::Name(BRAND.into()), [vec!["bricolage".to_string()], fallback.clone()].concat());
    let mut headline = fallback;
    if let Some(sf) = sf {
        let sf = FontData::from_owned(sf);
        f.font_data.insert("sf".into(), weight(sf.clone(), 400.0, None));
        f.font_data.insert("sf-semibold".into(), weight(sf, 600.0, None));
        f.families.entry(FontFamily::Proportional).or_default().insert(0, "sf".into());
        headline.insert(0, "sf-semibold".into());
    }
    f.families.insert(FontFamily::Name(HEADLINE.into()), headline);
    f
}

fn style(s: &mut egui::Style, p: &Palette) {
    use FontFamily::{Monospace, Proportional};
    // The macOS text styles: .title2, .body, .headline, .caption.
    s.text_styles = [
        (TextStyle::Heading, FontId::new(17.0, Proportional)),
        (TextStyle::Body, FontId::new(13.0, Proportional)),
        (TextStyle::Button, FontId::new(13.0, Proportional)),
        (TextStyle::Small, FontId::new(10.0, Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, Monospace)),
        (headline(), FontId::new(13.0, FontFamily::Name(HEADLINE.into()))),
    ]
    .into();
    s.spacing.item_spacing = Vec2::new(8.0, 4.0);
    s.spacing.button_padding = Vec2::new(8.0, 4.0);
    visuals(&mut s.visuals, p);
}

fn visuals(v: &mut Visuals, p: &Palette) {
    v.panel_fill = p.window;
    v.window_fill = p.surface;
    v.faint_bg_color = p.surface;
    // Behind progress bars; text fields keep the surface.
    v.extreme_bg_color = p.free;
    v.text_edit_bg_color = Some(p.surface);
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.text_secondary);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(6);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
    }
    v.widgets.inactive.weak_bg_fill = p.control;
    v.widgets.inactive.bg_fill = p.control;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.separator);
    v.widgets.hovered.weak_bg_fill = p.control_hover;
    v.widgets.hovered.bg_fill = p.control_hover;
}

/// The one main action of a view: Jaune Balai, Encre text, a thin edge in light mode.
pub fn primary_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    let edge = if ui.visuals().dark_mode { Stroke::NONE } else { Stroke::new(1.0, Color32::from_rgba_unmultiplied(22, 22, 22, 31)) };
    egui::Button::new(egui::RichText::new(text).text_style(headline()).color(INK))
        .fill(YELLOW)
        .stroke(edge)
        .corner_radius(CornerRadius::same(8))
        .min_size(Vec2::new(0.0, 32.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two opaque colours, from 1 to 21.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn luminance(c: Color32) -> f32 {
            let lin = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        }
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn every_text_token_reads_at_4_5_to_1() {
        for p in [&LIGHT, &DARK] {
            for bg in [p.window, p.surface] {
                for fg in [p.text, p.text_secondary] {
                    assert!(contrast(fg, bg) >= 4.5, "{fg:?} on {bg:?}: {:.2}", contrast(fg, bg));
                }
            }
            // The design document rates the accent on bg.surface; on the dark window it is 4.0:1.
            assert!(contrast(p.accent, p.surface) >= 4.5);
            assert!(contrast(p.text, p.control) >= 4.5);
        }
        assert!(contrast(INK, YELLOW) >= 4.5);
        assert!(contrast(CREAM, INK) >= 4.5);
    }

    #[test]
    fn contrast_matches_the_design_document() {
        assert!((contrast(LIGHT.text, LIGHT.surface) - 18.1).abs() < 0.1);
        assert!((contrast(INK, YELLOW) - 12.7).abs() < 0.1);
    }

    #[test]
    fn the_themes_carry_the_tokens() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for (theme, p) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
            ctx.set_theme(theme);
            let v = ctx.global_style().visuals.clone();
            assert_eq!(v.panel_fill, p.window);
            assert_eq!(v.faint_bg_color, p.surface);
            assert_eq!(v.selection.stroke.color, p.accent);
            assert_eq!(v.override_text_color, Some(p.text));
        }
    }

    #[test]
    fn without_sf_the_families_fall_back_to_egui() {
        let f = fonts(None);
        let proportional = &f.families[&FontFamily::Proportional];
        assert!(!proportional.contains(&"sf".to_string()));
        assert_eq!(&f.families[&FontFamily::Name(HEADLINE.into())], proportional);
        assert_eq!(f.families[&FontFamily::Name(BRAND.into())][0], "bricolage");
    }

    #[test]
    fn with_sf_the_interface_uses_it_first() {
        let Ok(sf) = std::fs::read(SF) else { return };
        let f = fonts(Some(sf));
        assert_eq!(f.families[&FontFamily::Proportional][0], "sf");
        assert_eq!(f.families[&FontFamily::Name(HEADLINE.into())][0], "sf-semibold");
    }
}
