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

/// The interface tokens of one appearance, as the product page (`site/style.css`) uses them.
pub struct Palette {
    /// The page's background: cream, or ink in the dark.
    pub window: Color32,
    pub surface: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub accent: Color32,
    /// A thin divider inside a surface.
    pub separator: Color32,
    /// The heavy line of the page: card edges, their hard shadow, the controls' outline.
    pub line: Color32,
    /// The sidebar, the text on it, then its current entry and the text on that.
    pub sidebar: Color32,
    pub on_sidebar: Color32,
    pub current: Color32,
    pub on_current: Color32,
    /// The measures: the main colour, then the track behind a bar. Coral and yellow complete them.
    pub data: Color32,
    pub free: Color32,
    /// A fourth, quiet share in a stacked bar.
    pub data_quiet: Color32,
    /// A control at rest, hovered, then pressed. The text keeps its colour in every state: a
    /// checkbox label or a scrollbar handle is drawn with it and has no fill behind.
    pub control: Color32,
    pub control_hover: Color32,
    pub control_pressed: Color32,
    /// Selected text and the selected column of a table, with the text on it.
    pub selection: Color32,
    pub on_selection: Color32,
    pub error: Color32,
}

pub const LIGHT: Palette = Palette {
    window: CREAM,
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    text: INK,
    text_secondary: Color32::from_rgb(0x5E, 0x5C, 0x55),
    accent: Color32::from_rgb(0xC2, 0x3A, 0x12),
    separator: Color32::from_rgb(0xD9, 0xD9, 0xD9),
    line: INK,
    sidebar: YELLOW,
    on_sidebar: INK,
    current: INK,
    on_current: YELLOW,
    data: INK,
    free: Color32::from_rgb(0xF3, 0xEB, 0xC8),
    data_quiet: Color32::from_rgb(0xB9, 0xB6, 0xAC),
    control: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    control_hover: YELLOW,
    control_pressed: CORAL,
    selection: YELLOW,
    on_selection: INK,
    error: Color32::from_rgb(0xC2, 0x3A, 0x12),
};

pub const DARK: Palette = Palette {
    window: INK,
    surface: Color32::from_rgb(0x22, 0x22, 0x20),
    text: CREAM,
    text_secondary: Color32::from_rgb(0xB9, 0xB6, 0xAC),
    accent: YELLOW,
    separator: Color32::from_rgb(0x3A, 0x3A, 0x38),
    line: CREAM,
    sidebar: Color32::from_rgb(0x0D, 0x0D, 0x0D),
    on_sidebar: CREAM,
    current: YELLOW,
    on_current: INK,
    data: CREAM,
    free: Color32::from_rgb(0x3A, 0x3A, 0x38),
    data_quiet: Color32::from_rgb(0x8A, 0x87, 0x7E),
    control: Color32::from_rgb(0x2A, 0x2A, 0x28),
    control_hover: Color32::from_rgb(0xB0, 0x3C, 0x1E),
    control_pressed: Color32::from_rgb(0x3A, 0x3A, 0x38),
    selection: Color32::from_rgb(0x8A, 0x32, 0x1E),
    on_selection: CREAM,
    error: Color32::from_rgb(0xFF, 0x8A, 0x70),
};

pub fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

/// Bricolage Grotesque, a variable font (SIL Open Font License, see `assets/OFL.txt`).
const BRICOLAGE: &[u8] = include_bytes!("../assets/BricolageGrotesque.ttf");
/// The macOS interface font, read where the system keeps it: never shipped with the app.
const SF: &str = "/System/Library/Fonts/SFNS.ttf";
const SF_MONO: &str = "/System/Library/Fonts/SFNSMono.ttf";

/// Brand family: the logotype, the titles and the big figures, never below 18 pt.
pub const BRAND: &str = "brand";
/// A card's title, in the brand family.
pub const TITLE: &str = "title";
/// SF Semibold, the system `.headline`.
pub const HEADLINE: &str = "headline";

pub fn headline() -> TextStyle {
    TextStyle::Name(HEADLINE.into())
}

pub fn title() -> TextStyle {
    TextStyle::Name(TITLE.into())
}

/// A big figure of the dashboard.
pub fn figure_font() -> FontId {
    FontId::new(26.0, FontFamily::Name(BRAND.into()))
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

/// Corners: a card, a text field, and a control, which is a pill.
const CARD_RADIUS: u8 = 14;
const FIELD_RADIUS: u8 = 10;
const PILL: u8 = u8::MAX;

fn style(s: &mut egui::Style, p: &Palette) {
    use FontFamily::{Monospace, Proportional};
    let brand = || FontFamily::Name(BRAND.into());
    s.text_styles = [
        (TextStyle::Heading, FontId::new(30.0, brand())),
        (title(), FontId::new(19.0, brand())),
        (TextStyle::Body, FontId::new(13.0, Proportional)),
        (TextStyle::Button, FontId::new(13.0, Proportional)),
        (TextStyle::Small, FontId::new(10.0, Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, Monospace)),
        (headline(), FontId::new(13.0, FontFamily::Name(HEADLINE.into()))),
    ]
    .into();
    s.spacing.item_spacing = Vec2::new(8.0, 6.0);
    s.spacing.button_padding = Vec2::new(12.0, 5.0);
    visuals(&mut s.visuals, p);
}

fn visuals(v: &mut Visuals, p: &Palette) {
    v.panel_fill = p.window;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(2.0, p.line);
    v.faint_bg_color = p.surface;
    // Behind progress bars; text fields keep the surface.
    v.extreme_bg_color = p.free;
    v.text_edit_bg_color = Some(p.surface);
    v.override_text_color = None;
    v.weak_text_color = Some(p.text_secondary);
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.error;
    v.selection.bg_fill = p.selection;
    // Also the outline of a focused text field, heavier than the field at rest.
    v.selection.stroke = Stroke::new(2.0, p.on_selection);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.menu_corner_radius = CornerRadius::same(FIELD_RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = CornerRadius::same(FIELD_RADIUS);
    for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(PILL);
        s.bg_stroke = Stroke::new(1.5, p.line);
        s.fg_stroke = Stroke::new(1.0, p.text);
        s.weak_bg_fill = p.control;
        s.bg_fill = p.control;
        s.expansion = 0.0;
    }
    w.hovered.weak_bg_fill = p.control_hover;
    w.hovered.bg_fill = p.control_hover;
    w.active.weak_bg_fill = p.control_pressed;
    w.active.bg_fill = p.control_pressed;
}

/// A text field: the controls are pills, a field keeps squarer corners.
pub fn field<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = CornerRadius::same(FIELD_RADIUS);
        }
        add(ui)
    })
    .inner
}

/// The page's « stamp »: a surface with a heavy edge and a hard shadow, no blur.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(2.0, p.line))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(egui::Margin::same(16))
        // Room for the edge and the shadow, which the scroll area would cut otherwise.
        .outer_margin(egui::Margin { left: 2, top: 2, right: 6, bottom: 6 })
        .shadow(egui::Shadow { offset: [4, 4], blur: 0, spread: 0, color: p.line })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A small label in capitals above a group, as the page's eyebrows.
pub fn eyebrow(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let color = palette(ui).text_secondary;
    ui.label(egui::RichText::new(text.to_uppercase()).font(FontId::new(10.5, FontFamily::Name(HEADLINE.into()))).extra_letter_spacing(0.9).color(color))
}

/// The sidebar's own colours: no outline, the current entry a pill of the opposite colour.
pub fn sidebar(ui: &mut egui::Ui) {
    let p = palette(ui);
    let v = ui.visuals_mut();
    v.selection.bg_fill = p.current;
    v.selection.stroke = Stroke::new(2.0, p.on_current);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.on_sidebar);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.on_sidebar.gamma_multiply(0.25));
    for s in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        s.bg_stroke = Stroke::NONE;
        s.fg_stroke = Stroke::new(1.0, p.on_sidebar);
        s.weak_bg_fill = p.on_sidebar.gamma_multiply(0.12);
        s.bg_fill = s.weak_bg_fill;
    }
}

/// The one main action of a view, as the page's main button: an Encre pill with Jaune Balai text
/// and a hard Corail shadow; it lifts when hovered and sinks when pressed, and wears a ring when
/// the keyboard is on it. In the dark the pill is Jaune Balai with Encre text.
pub fn primary_button(text: &str) -> impl egui::Widget + '_ {
    /// How far the shadow reaches, and how far the pill lifts when hovered.
    const SHADOW: f32 = 4.0;
    const LIFT: f32 = 2.0;
    move |ui: &mut egui::Ui| {
        let (fill, ink) = if ui.visuals().dark_mode { (YELLOW, INK) } else { (INK, YELLOW) };
        let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, FontFamily::Name(HEADLINE.into())), ink);
        let pill = galley.size() + Vec2::new(36.0, 16.0);
        let (rect, response) = ui.allocate_exact_size(pill + Vec2::splat(SHADOW + LIFT), egui::Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text));
        if ui.is_rect_visible(rect) {
            let offset = if response.is_pointer_button_down_on() {
                LIFT + SHADOW
            } else if response.hovered() {
                0.0
            } else {
                LIFT
            };
            let body = egui::Rect::from_min_size(rect.min + Vec2::splat(offset), pill);
            let shadow = egui::Rect::from_min_size(rect.min + Vec2::splat(LIFT + SHADOW), pill);
            let round = CornerRadius::same(PILL);
            ui.painter().rect_filled(shadow, round, CORAL);
            ui.painter().rect_filled(body, round, fill);
            if response.has_focus() {
                ui.painter().rect_stroke(body, round, Stroke::new(2.0, CORAL), egui::StrokeKind::Outside);
            }
            ui.painter().galley(body.center() - galley.size() / 2.0, galley, ink);
        }
        response
    }
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
    fn every_text_on_its_background_reads_at_4_5_to_1() {
        for p in [&LIGHT, &DARK] {
            let pairs = [
                (p.text, p.window),
                (p.text, p.surface),
                (p.text_secondary, p.window),
                (p.text_secondary, p.surface),
                (p.accent, p.window),
                (p.accent, p.surface),
                (p.error, p.window),
                (p.error, p.surface),
                (p.on_sidebar, p.sidebar),
                (p.on_current, p.current),
                (p.text, p.control),
                (p.text, p.control_hover),
                (p.text, p.control_pressed),
                (p.on_selection, p.selection),
            ];
            for (fg, bg) in pairs {
                assert!(contrast(fg, bg) >= 4.5, "{fg:?} on {bg:?}: {:.2}", contrast(fg, bg));
            }
        }
        // The main button, in both appearances.
        assert!(contrast(INK, YELLOW) >= 4.5);
        assert!(contrast(YELLOW, INK) >= 4.5);
        assert!(contrast(CREAM, INK) >= 4.5);
    }

    #[test]
    fn contrast_matches_the_design_document() {
        assert!((contrast(LIGHT.text, LIGHT.surface) - 18.1).abs() < 0.1);
        assert!((contrast(INK, YELLOW) - 12.7).abs() < 0.1);
    }

    #[test]
    fn the_themes_carry_the_page_tokens() {
        assert_eq!((LIGHT.window, LIGHT.sidebar, LIGHT.line, LIGHT.surface), (CREAM, YELLOW, INK, Color32::WHITE));
        assert_eq!((DARK.window, DARK.line, DARK.text), (INK, CREAM, CREAM));
        let ctx = egui::Context::default();
        apply(&ctx);
        for (theme, p) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
            ctx.set_theme(theme);
            let v = ctx.global_style().visuals.clone();
            assert_eq!(v.panel_fill, p.window);
            assert_eq!(v.faint_bg_color, p.surface);
            assert_eq!(v.text_color(), p.text);
            assert_eq!(v.strong_text_color(), p.text);
            assert_eq!(v.selection.bg_fill, p.selection);
            // A control is a pill outlined with the heavy line.
            assert_eq!(v.widgets.inactive.corner_radius, CornerRadius::same(PILL));
            assert_eq!(v.widgets.inactive.bg_stroke, Stroke::new(1.5, p.line));
            assert_eq!(v.widgets.hovered.weak_bg_fill, p.control_hover);
            // A checkbox label, a folded header and a scrollbar handle have no fill of their own:
            // in every state their colour must read on the window and on a card.
            for state in [v.widgets.inactive, v.widgets.hovered, v.widgets.active, v.widgets.open] {
                for bg in [p.window, p.surface] {
                    assert!(contrast(state.fg_stroke.color, bg) >= 4.5, "{:?} on {bg:?}", state.fg_stroke.color);
                }
            }
            // A focused text field is outlined at least as heavily as one at rest.
            assert!(v.selection.stroke.width >= v.widgets.inactive.bg_stroke.width);
        }
    }

    #[test]
    fn titles_use_the_brand_font() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let styles = ctx.global_style().text_styles.clone();
        for style in [TextStyle::Heading, title()] {
            assert_eq!(styles[&style].family, FontFamily::Name(BRAND.into()));
            assert!(styles[&style].size >= 18.0);
        }
        assert_eq!(figure_font().family, FontFamily::Name(BRAND.into()));
    }

    #[test]
    fn the_measures_use_brand_colours_only() {
        for p in [&LIGHT, &DARK] {
            assert!([INK, CREAM].contains(&p.data), "{:?}", p.data);
            // A bar and a curve stand out from their track.
            assert!(contrast(p.data, p.free) >= 3.0);
            for c in [p.data, p.free, p.data_quiet] {
                // No hue of its own: nothing bluer than it is red.
                assert!(c.b() <= c.r(), "{c:?}");
            }
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
