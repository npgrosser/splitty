use gpui::{
    FontFallbacks, FontId, FontRun, FontStyle, FontWeight, PlatformTextSystem, RenderGlyphParams,
    font, point, px,
};
use gpui_wgpu::CosmicTextSystem;
use std::borrow::Cow;

fn system() -> CosmicTextSystem {
    let system = CosmicTextSystem::new_without_system_fonts("Splitty Variable Test");
    system
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("fonts/variable.ttf")),
            Cow::Borrowed(include_bytes!("fonts/fallback.ttf")),
            Cow::Borrowed(include_bytes!("fonts/static.ttf")),
        ])
        .unwrap();
    system
}

fn ink(system: &CosmicTextSystem, id: FontId, ch: char) -> u64 {
    let params = RenderGlyphParams {
        font_id: id,
        glyph_id: system.glyph_for_char(id, ch).unwrap(),
        font_size: px(40.),
        subpixel_variant: point(0, 0),
        scale_factor: 1.,
        is_emoji: false,
        subpixel_rendering: false,
        dilation: 0,
    };
    let bounds = system.glyph_raster_bounds(&params).unwrap();
    system
        .rasterize_glyph(&params, bounds)
        .unwrap()
        .1
        .into_iter()
        .map(u64::from)
        .sum()
}

#[test]
fn variable_weights_reach_matching_metrics_shaping_and_rasterization() {
    let system = system();
    let regular = system.font_id(&font("Splitty Variable Test")).unwrap();
    let bold_request = font("Splitty Variable Test").bold();
    let bold = system.font_id(&bold_request).unwrap();
    assert_ne!(regular, bold);
    assert_eq!(system.font_id(&bold_request).unwrap(), bold);
    assert_eq!(
        system.font_weight_and_style(bold).unwrap(),
        (FontWeight::BOLD, FontStyle::Normal)
    );
    let glyph = system.glyph_for_char(regular, 'm').unwrap();
    assert_eq!(system.advance(regular, glyph).unwrap().width, 500.);
    assert_eq!(system.advance(bold, glyph).unwrap().width, 800.);
    assert_eq!(
        system.typographic_bounds(bold, glyph).unwrap().size.width,
        800.
    );
    let medium = system
        .font_id(&gpui::Font {
            weight: FontWeight(550.),
            ..bold_request.clone()
        })
        .unwrap();
    for (id, expected) in [(regular, 40.), (medium, 52.), (bold, 64.)] {
        let line = system.layout_line(
            "mm",
            px(40.),
            &[FontRun {
                len: 2,
                font_id: id,
            }],
        );
        assert!((f32::from(line.width) - expected).abs() < 0.01);
        assert_eq!(line.runs[0].font_id, id);
    }
    assert!(ink(&system, bold, 'm') > ink(&system, regular, 'm') * 3 / 2);
    // Rendering Bold must not change a cached Regular instance.
    assert_eq!(
        system.font_id(&font("Splitty Variable Test")).unwrap(),
        regular
    );
    assert_eq!(system.advance(regular, glyph).unwrap().width, 500.);
    let heavy = gpui::Font {
        weight: FontWeight::BLACK,
        ..bold_request
    };
    assert_eq!(system.font_id(&heavy).unwrap(), bold); // clamp to the declared axis range
}

#[test]
fn variable_fallback_keeps_the_shaped_weight_in_its_raster_font() {
    for explicit in [false, true] {
        let system = system();
        let primary = gpui::Font {
            fallbacks: explicit
                .then(|| FontFallbacks::from_fonts(vec!["Splitty Fallback Test".into()])),
            ..font("Splitty Variable Test").bold()
        };
        let id = system.font_id(&primary).unwrap();
        let text = "mΩ";
        let line = system.layout_line(
            text,
            px(40.),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        );
        assert_eq!(line.runs.len(), 2);
        let fallback = line.runs[1].font_id;
        assert_ne!(fallback, id);
        assert_eq!(
            system.font_weight_and_style(fallback).unwrap().0,
            FontWeight::BOLD
        );
        assert!((f32::from(line.width) - 64.).abs() < 0.01);
        let regular_fallback = system.font_id(&font("Splitty Fallback Test")).unwrap();
        assert!(ink(&system, fallback, 'Ω') > ink(&system, regular_fallback, 'Ω') * 3 / 2);
    }
}

#[test]
fn static_faces_keep_their_actual_weight_and_identity() {
    let system = system();
    let regular = system.font_id(&font("Splitty Static Test")).unwrap();
    let bold = system.font_id(&font("Splitty Static Test").bold()).unwrap();
    assert_eq!(regular, bold);
    assert_eq!(
        system.font_weight_and_style(bold).unwrap().0,
        FontWeight::NORMAL
    );
}
