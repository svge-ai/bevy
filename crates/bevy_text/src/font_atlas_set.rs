use crate::{Font, FontAtlas, FontSmoothing, TextFont};
use bevy_asset::{AssetEvent, AssetId};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{message::MessageReader, resource::Resource, system::ResMut};
use bevy_platform::collections::HashMap;

/// Horizontal-subpixel bucket that partitions the subpixel glyph cache.
///
/// With `FontSmoothing::SubpixelAntiAliased`, glyphs rasterised at different
/// horizontal subpixel offsets (e.g. `x=10.1` vs `x=10.4`) yield visually
/// different pixel data and so must be cached separately. `cosmic_text`
/// already quantises every glyph's sub-pixel position into a
/// [`cosmic_text::SubpixelBin`] with four variants; we mirror those four
/// variants here plus a `NotApplicable` case used by the non-subpixel
/// smoothing modes (where the bucket is a no-op).
///
/// Without this partition, a single glyph animating across the screen would
/// evict its own atlas entry every frame because the `FontAtlasKey` ignored
/// the subpixel offset.
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy, Default)]
pub enum SubpixelBucket {
    /// Used for `FontSmoothing::None` and `FontSmoothing::AntiAliased`, where
    /// the rasterised glyph is independent of horizontal subpixel position.
    #[default]
    NotApplicable,
    /// Corresponds to [`cosmic_text::SubpixelBin::Zero`].
    BinZero,
    /// Corresponds to [`cosmic_text::SubpixelBin::One`].
    BinOne,
    /// Corresponds to [`cosmic_text::SubpixelBin::Two`].
    BinTwo,
    /// Corresponds to [`cosmic_text::SubpixelBin::Three`].
    BinThree,
}

impl From<cosmic_text::SubpixelBin> for SubpixelBucket {
    fn from(bin: cosmic_text::SubpixelBin) -> Self {
        match bin {
            cosmic_text::SubpixelBin::Zero => SubpixelBucket::BinZero,
            cosmic_text::SubpixelBin::One => SubpixelBucket::BinOne,
            cosmic_text::SubpixelBin::Two => SubpixelBucket::BinTwo,
            cosmic_text::SubpixelBin::Three => SubpixelBucket::BinThree,
        }
    }
}

/// Identifies the font atlases for a particular font in [`FontAtlasSet`].
///
/// Allows an `f32` font size to be used as a key in a `HashMap`, by its binary
/// representation. For `FontSmoothing::SubpixelAntiAliased`, the
/// [`subpixel_bucket`](Self::subpixel_bucket) also partitions atlases by
/// horizontal subpixel offset; for the other smoothing modes it is always
/// [`SubpixelBucket::NotApplicable`].
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct FontAtlasKey {
    /// The font asset this atlas renders glyphs from.
    pub font: AssetId<Font>,
    /// Font size, stored as an `f32`'s bit pattern so it can be hashed.
    pub size: u32,
    /// Smoothing strategy for the rasterised glyphs.
    pub smoothing: FontSmoothing,
    /// Horizontal-subpixel bucket. Meaningful only when
    /// `smoothing == FontSmoothing::SubpixelAntiAliased`; otherwise always
    /// [`SubpixelBucket::NotApplicable`].
    pub subpixel_bucket: SubpixelBucket,
}

impl From<&TextFont> for FontAtlasKey {
    fn from(font: &TextFont) -> Self {
        FontAtlasKey {
            font: font.font.id(),
            size: font.font_size.to_bits(),
            smoothing: font.font_smoothing,
            // `From<&TextFont>` is used by lookups that do not carry a
            // subpixel-offset context (e.g. invalidating every atlas for a
            // given font size). The `SubpixelAntiAliased` path constructs
            // `FontAtlasKey` directly with the correct bucket in
            // `bevy_text::pipeline`.
            subpixel_bucket: SubpixelBucket::NotApplicable,
        }
    }
}

/// Set of rasterized fonts stored in [`FontAtlas`]es.
#[derive(Debug, Default, Resource, Deref, DerefMut)]
pub struct FontAtlasSet(HashMap<FontAtlasKey, Vec<FontAtlas>>);

impl FontAtlasSet {
    /// Checks whether the given subpixel-offset glyph is contained in any of the [`FontAtlas`]es for the font identified by the given [`FontAtlasKey`].
    pub fn has_glyph(&self, cache_key: cosmic_text::CacheKey, font_key: &FontAtlasKey) -> bool {
        self.get(font_key)
            .is_some_and(|font_atlas| font_atlas.iter().any(|atlas| atlas.has_glyph(cache_key)))
    }
}

/// A system that automatically frees unused texture atlases when a font asset is removed.
pub fn free_unused_font_atlases_system(
    mut font_atlas_sets: ResMut<FontAtlasSet>,
    mut font_events: MessageReader<AssetEvent<Font>>,
) {
    for event in font_events.read() {
        if let AssetEvent::Removed { id } = event {
            font_atlas_sets.retain(|key, _| key.font != *id);
        }
    }
}
