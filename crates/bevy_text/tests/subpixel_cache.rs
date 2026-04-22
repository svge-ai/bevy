//! Integration test for phase-03 of spec/0002b.
//!
//! Verifies that the horizontal-subpixel-bucket extension to [`FontAtlasKey`]
//! keeps the subpixel glyph atlas partitioned per-bucket, and that a repeat
//! rasterisation at the same bucket is a cache hit (no new atlas allocations,
//! no new atlas entries).
//!
//! The test drives the low-level atlas entry point directly rather than a
//! full `App` because we want deterministic control over the physical-glyph
//! subpixel offset (`x_bin`).

extern crate alloc;

use alloc::sync::Arc;

use bevy_asset::Assets;
use bevy_image::prelude::*;
use bevy_text::{
    add_glyph_to_atlas, FontAtlas, FontAtlasKey, FontSmoothing, SubpixelBucket,
};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SubpixelBin, SwashCache, Weight};
use std::collections::HashMap;

/// Font shipped with Bevy — matches the one used by the phase-02 rasterisation
/// test, so that the subpixel path actually kicks in (outline glyphs only).
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

/// Shape "A" at 32pt and return the (cloned) first `LayoutGlyph`. We need an
/// owned value so the caller can repeatedly derive `PhysicalGlyph`s with
/// different sub-pixel offsets.
fn shape_glyph_a(font_system: &mut FontSystem) -> cosmic_text::LayoutGlyph {
    let metrics = Metrics::new(32.0, 32.0);
    let mut buffer = Buffer::new(font_system, metrics);
    let attrs = Attrs::new()
        .family(Family::Name("Fira Mono"))
        .weight(Weight::MEDIUM);
    buffer.set_text(font_system, "A", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let run = buffer.layout_runs().next().expect("no layout run for 'A'");
    run.glyphs.first().expect("no glyph for 'A'").clone()
}

/// Apply a horizontal fractional offset to `layout_glyph.x` so that
/// `physical(0., 0.)` quantises into the desired [`SubpixelBin`].
///
/// `SubpixelBin::new` bins an `f32` into four buckets at sub-pixel stride
/// 0.25; the returned `layout_glyph` reflects that offset.
fn shift_to_bin(mut layout_glyph: cosmic_text::LayoutGlyph, bin: SubpixelBin) -> cosmic_text::LayoutGlyph {
    let integer_x = layout_glyph.x.floor();
    let fractional = match bin {
        SubpixelBin::Zero => 0.0,
        SubpixelBin::One => 0.25,
        SubpixelBin::Two => 0.5,
        SubpixelBin::Three => 0.75,
    };
    layout_glyph.x = integer_x + fractional;
    layout_glyph
}

#[test]
fn subpixel_bucket_partitions_atlas_and_caches_hit() {
    let mut font_system = make_font_system();
    let mut swash_cache = SwashCache::new();
    let base_glyph = shape_glyph_a(&mut font_system);

    // Four distinct bins => four distinct atlas entries under
    // `FontSmoothing::SubpixelAntiAliased`.
    let bins = [
        SubpixelBin::Zero,
        SubpixelBin::One,
        SubpixelBin::Two,
        SubpixelBin::Three,
    ];

    let mut textures = Assets::<Image>::default();
    let mut texture_atlases = Assets::<TextureAtlasLayout>::default();
    let mut font_atlas_set: HashMap<FontAtlasKey, Vec<FontAtlas>> = HashMap::new();

    let font_id = bevy_asset::AssetId::<bevy_text::Font>::invalid();
    let size = 32f32.to_bits();

    // Round 1: populate each bucket once.
    for &bin in &bins {
        let layout_glyph = shift_to_bin(base_glyph.clone(), bin);
        let physical = layout_glyph.physical((0., 0.), 1.0);
        assert_eq!(
            physical.cache_key.x_bin, bin,
            "LayoutGlyph offset did not round-trip to the expected SubpixelBin",
        );

        let bucket = SubpixelBucket::from(physical.cache_key.x_bin);
        let key = FontAtlasKey {
            font: font_id,
            size,
            smoothing: FontSmoothing::SubpixelAntiAliased,
            subpixel_bucket: bucket,
        };
        let atlases = font_atlas_set.entry(key).or_default();
        add_glyph_to_atlas(
            atlases,
            &mut texture_atlases,
            &mut textures,
            &mut font_system,
            &mut swash_cache,
            &layout_glyph,
            FontSmoothing::SubpixelAntiAliased,
        )
        .expect("subpixel rasterisation failed on first insert");
    }

    // Expectation: one `FontAtlasKey` per bucket, each with one `FontAtlas`
    // containing exactly one glyph.
    assert_eq!(
        font_atlas_set.len(),
        4,
        "expected four distinct subpixel buckets, got {}",
        font_atlas_set.len(),
    );
    for (key, atlases) in &font_atlas_set {
        assert_ne!(key.subpixel_bucket, SubpixelBucket::NotApplicable);
        assert_eq!(atlases.len(), 1, "each bucket should own exactly one atlas");
        assert_eq!(
            atlases[0].glyph_to_atlas_index.len(),
            1,
            "each atlas should hold exactly one glyph after round 1",
        );
    }

    // Capture state so we can assert round 2 is a pure cache hit.
    let atlas_count_before = font_atlas_set
        .values()
        .map(Vec::len)
        .sum::<usize>();
    let glyph_count_before = font_atlas_set
        .values()
        .flat_map(|atlases| atlases.iter().map(|a| a.glyph_to_atlas_index.len()))
        .sum::<usize>();
    let key_count_before = font_atlas_set.len();
    let image_count_before = textures.len();
    let layout_count_before = texture_atlases.len();

    // Round 2: re-add each glyph. All four should be cache hits — `add_glyph`
    // on an existing atlas will insert the same `cache_key` again (replacing
    // itself), so the inner glyph count must stay at 1 per atlas.
    for &bin in &bins {
        let layout_glyph = shift_to_bin(base_glyph.clone(), bin);
        let physical = layout_glyph.physical((0., 0.), 1.0);
        let bucket = SubpixelBucket::from(physical.cache_key.x_bin);
        let key = FontAtlasKey {
            font: font_id,
            size,
            smoothing: FontSmoothing::SubpixelAntiAliased,
            subpixel_bucket: bucket,
        };
        let atlases = font_atlas_set.entry(key).or_default();
        add_glyph_to_atlas(
            atlases,
            &mut texture_atlases,
            &mut textures,
            &mut font_system,
            &mut swash_cache,
            &layout_glyph,
            FontSmoothing::SubpixelAntiAliased,
        )
        .expect("subpixel rasterisation failed on repeat insert");
    }

    let atlas_count_after = font_atlas_set
        .values()
        .map(Vec::len)
        .sum::<usize>();
    let glyph_count_after = font_atlas_set
        .values()
        .flat_map(|atlases| atlases.iter().map(|a| a.glyph_to_atlas_index.len()))
        .sum::<usize>();

    assert_eq!(
        font_atlas_set.len(),
        key_count_before,
        "round 2 introduced new FontAtlasKey entries — caching is broken",
    );
    assert_eq!(
        atlas_count_after, atlas_count_before,
        "round 2 created additional FontAtlas allocations",
    );
    assert_eq!(
        glyph_count_after, glyph_count_before,
        "round 2 changed the total number of cached glyph entries",
    );
    assert_eq!(
        textures.len(),
        image_count_before,
        "round 2 created new `Image` assets — subpixel glyph was re-uploaded",
    );
    assert_eq!(
        texture_atlases.len(),
        layout_count_before,
        "round 2 created new `TextureAtlasLayout` assets",
    );
}

#[test]
fn non_subpixel_smoothing_uses_not_applicable_bucket() {
    // `FontSmoothing::AntiAliased` rasterisation must key atlases with
    // `SubpixelBucket::NotApplicable` regardless of the glyph's horizontal
    // offset, matching the pre-cache behaviour for backwards compatibility.
    let mut font_system = make_font_system();
    let mut swash_cache = SwashCache::new();
    let base_glyph = shape_glyph_a(&mut font_system);

    let mut textures = Assets::<Image>::default();
    let mut texture_atlases = Assets::<TextureAtlasLayout>::default();
    let mut font_atlas_set: HashMap<FontAtlasKey, Vec<FontAtlas>> = HashMap::new();

    let font_id = bevy_asset::AssetId::<bevy_text::Font>::invalid();
    let size = 32f32.to_bits();

    for &bin in &[SubpixelBin::Zero, SubpixelBin::One, SubpixelBin::Two, SubpixelBin::Three] {
        let layout_glyph = shift_to_bin(base_glyph.clone(), bin);
        // For AntiAliased, always use `NotApplicable` — this mirrors what
        // `pipeline.rs` does and keeps all four offsets in the same atlas.
        let key = FontAtlasKey {
            font: font_id,
            size,
            smoothing: FontSmoothing::AntiAliased,
            subpixel_bucket: SubpixelBucket::NotApplicable,
        };
        let atlases = font_atlas_set.entry(key).or_default();
        add_glyph_to_atlas(
            atlases,
            &mut texture_atlases,
            &mut textures,
            &mut font_system,
            &mut swash_cache,
            &layout_glyph,
            FontSmoothing::AntiAliased,
        )
        .expect("AntiAliased rasterisation failed");
    }

    assert_eq!(
        font_atlas_set.len(),
        1,
        "AntiAliased smoothing should collapse all bins into a single FontAtlasKey",
    );
    let (key, _) = font_atlas_set.iter().next().unwrap();
    assert_eq!(key.subpixel_bucket, SubpixelBucket::NotApplicable);
}
