//! Integration test for phase-02 of spec 0002.
//!
//! Verifies that `get_outlined_glyph_texture` under `FontSmoothing::SubpixelAntiAliased`
//! produces RGB-distinguishable output — at least one pixel where R / G / B
//! are not all equal. The grayscale path emits `[255, 255, 255, a]` so
//! `R == G == B` always.
//!
//! Implemented as an integration test (rather than an in-crate unit test)
//! because the subpixel rasterisation entry point is private
//! (`rasterise_subpixel_glyph`); we exercise it indirectly through the public
//! `get_outlined_glyph_texture`, which requires a fully wired-up
//! `cosmic_text::FontSystem` / `SwashCache` / `PhysicalGlyph`.

use std::sync::Arc;

use bevy_text::{get_outlined_glyph_texture, FontSmoothing};
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight,
};

/// Font shipped with Bevy, used here because it exercises the scalable
/// outline path (where subpixel rasterisation actually differs from
/// grayscale).
const FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/FiraMono-Medium.ttf");

fn make_font_system() -> FontSystem {
    let mut font_system = FontSystem::new_with_locale_and_db(
        "en-US".to_string(),
        cosmic_text::fontdb::Database::new(),
    );
    font_system
        .db_mut()
        .load_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(
            FONT_BYTES.to_vec(),
        )));
    font_system
}

/// Shape "A" at 32pt with the given font system and return the first glyph's
/// `PhysicalGlyph`. Panics if no glyph is produced (indicates a setup bug).
fn first_physical_glyph(font_system: &mut FontSystem) -> cosmic_text::PhysicalGlyph {
    let metrics = Metrics::new(32.0, 32.0);
    let mut buffer = Buffer::new(font_system, metrics);
    let attrs = Attrs::new()
        .family(Family::Name("Fira Mono"))
        .weight(Weight::MEDIUM);
    buffer.set_text(font_system, "A", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let run = buffer.layout_runs().next().expect("no layout run for 'A'");
    let glyph = run.glyphs.first().expect("no glyph for 'A'");
    glyph.physical((0.0, 0.0), 1.0)
}

#[test]
fn subpixel_rasterisation_produces_non_grayscale_output() {
    let mut font_system = make_font_system();
    let mut swash_cache = SwashCache::new();
    let physical_glyph = first_physical_glyph(&mut font_system);

    let (grayscale_image, _) = get_outlined_glyph_texture(
        &mut font_system,
        &mut swash_cache,
        &physical_glyph,
        FontSmoothing::AntiAliased,
    )
    .expect("grayscale rasterisation failed");

    let (subpixel_image, _) = get_outlined_glyph_texture(
        &mut font_system,
        &mut swash_cache,
        &physical_glyph,
        FontSmoothing::SubpixelAntiAliased,
    )
    .expect("subpixel rasterisation failed");

    let grayscale_data = grayscale_image
        .data
        .as_ref()
        .expect("grayscale image missing CPU data");
    let subpixel_data = subpixel_image
        .data
        .as_ref()
        .expect("subpixel image missing CPU data");

    // Grayscale: every pixel is [255, 255, 255, a] -> R == G == B for all pixels.
    assert!(
        grayscale_data
            .chunks_exact(4)
            .all(|px| px[0] == px[1] && px[1] == px[2]),
        "grayscale AntiAliased output unexpectedly had differing R/G/B",
    );

    // Subpixel: at least one pixel should have R != G, G != B, or R != B.
    // Equivalently, not all pixels satisfy R == G == B.
    let any_rgb_differs = subpixel_data
        .chunks_exact(4)
        .any(|px| px[0] != px[1] || px[1] != px[2]);
    assert!(
        any_rgb_differs,
        "subpixel output had uniform R==G==B across every pixel — the \
         Format::Subpixel rasterisation path does not appear to be active",
    );

    // Sanity: subpixel alpha channel must be 255 (alpha is encoded into RGB).
    assert!(
        subpixel_data.chunks_exact(4).all(|px| px[3] == 255),
        "subpixel alpha channel was not uniformly 255",
    );
}
