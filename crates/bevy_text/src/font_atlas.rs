use bevy_asset::{Assets, Handle, RenderAssetUsages};
use bevy_image::{prelude::*, ImageSampler, ToExtents};
use bevy_math::{IVec2, UVec2};
use bevy_platform::collections::HashMap;
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

use crate::{FontSmoothing, GlyphAtlasInfo, GlyphAtlasLocation, TextError};

/// Rasterized glyphs are cached, stored in, and retrieved from, a `FontAtlas`.
///
/// A `FontAtlas` contains one or more textures, each of which contains one or more glyphs packed into them.
///
/// A [`FontAtlasSet`](crate::FontAtlasSet) contains a `FontAtlas` for each font size in the same font face.
///
/// For the same font face and font size, a glyph will be rasterized differently for different subpixel offsets.
/// In practice, ranges of subpixel offsets are grouped into subpixel bins to limit the number of rasterized glyphs,
/// providing a trade-off between visual quality and performance.
///
/// A [`CacheKey`](cosmic_text::CacheKey) encodes all of the information of a subpixel-offset glyph and is used to
/// find that glyphs raster in a [`TextureAtlas`] through its corresponding [`GlyphAtlasLocation`].
pub struct FontAtlas {
    /// Used to update the [`TextureAtlasLayout`].
    pub dynamic_texture_atlas_builder: DynamicTextureAtlasBuilder,
    /// A mapping between subpixel-offset glyphs and their [`GlyphAtlasLocation`].
    pub glyph_to_atlas_index: HashMap<cosmic_text::CacheKey, GlyphAtlasLocation>,
    /// The handle to the [`TextureAtlasLayout`] that holds the rasterized glyphs.
    pub texture_atlas: Handle<TextureAtlasLayout>,
    /// The texture where this font atlas is located
    pub texture: Handle<Image>,
}

impl FontAtlas {
    /// Create a new [`FontAtlas`] with the given size, adding it to the appropriate asset collections.
    pub fn new(
        textures: &mut Assets<Image>,
        texture_atlases_layout: &mut Assets<TextureAtlasLayout>,
        size: UVec2,
        font_smoothing: FontSmoothing,
    ) -> FontAtlas {
        let mut image = Image::new_fill(
            size.to_extents(),
            TextureDimension::D2,
            &[0, 0, 0, 0],
            TextureFormat::Rgba8UnormSrgb,
            // Need to keep this image CPU persistent in order to add additional glyphs later on
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        if font_smoothing == FontSmoothing::None {
            image.sampler = ImageSampler::nearest();
        }
        let texture = textures.add(image);
        let texture_atlas = texture_atlases_layout.add(TextureAtlasLayout::new_empty(size));
        Self {
            texture_atlas,
            glyph_to_atlas_index: HashMap::default(),
            dynamic_texture_atlas_builder: DynamicTextureAtlasBuilder::new(size, 2),
            texture,
        }
    }

    /// Get the [`GlyphAtlasLocation`] for a subpixel-offset glyph.
    pub fn get_glyph_index(&self, cache_key: cosmic_text::CacheKey) -> Option<GlyphAtlasLocation> {
        self.glyph_to_atlas_index.get(&cache_key).copied()
    }

    /// Checks if the given subpixel-offset glyph is contained in this [`FontAtlas`].
    pub fn has_glyph(&self, cache_key: cosmic_text::CacheKey) -> bool {
        self.glyph_to_atlas_index.contains_key(&cache_key)
    }

    /// Add a glyph to the atlas, updating both its texture and layout.
    ///
    /// The glyph is represented by `glyph`, and its image content is `glyph_texture`.
    /// This content is copied into the atlas texture, and the atlas layout is updated
    /// to store the location of that glyph into the atlas.
    ///
    /// # Returns
    ///
    /// Returns `()` if the glyph is successfully added, or [`TextError::FailedToAddGlyph`] otherwise.
    /// In that case, neither the atlas texture nor the atlas layout are
    /// modified.
    pub fn add_glyph(
        &mut self,
        textures: &mut Assets<Image>,
        atlas_layouts: &mut Assets<TextureAtlasLayout>,
        cache_key: cosmic_text::CacheKey,
        texture: &Image,
        offset: IVec2,
    ) -> Result<(), TextError> {
        let atlas_layout = atlas_layouts
            .get_mut(&self.texture_atlas)
            .ok_or(TextError::MissingAtlasLayout)?;
        let atlas_texture = textures
            .get_mut(&self.texture)
            .ok_or(TextError::MissingAtlasTexture)?;

        if let Ok(glyph_index) =
            self.dynamic_texture_atlas_builder
                .add_texture(atlas_layout, texture, atlas_texture)
        {
            self.glyph_to_atlas_index.insert(
                cache_key,
                GlyphAtlasLocation {
                    glyph_index,
                    offset,
                },
            );
            Ok(())
        } else {
            Err(TextError::FailedToAddGlyph(cache_key.glyph_id))
        }
    }
}

impl core::fmt::Debug for FontAtlas {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FontAtlas")
            .field("glyph_to_atlas_index", &self.glyph_to_atlas_index)
            .field("texture_atlas", &self.texture_atlas)
            .field("texture", &self.texture)
            .field("dynamic_texture_atlas_builder", &"[...]")
            .finish()
    }
}

/// Adds the given subpixel-offset glyph to the given font atlases
pub fn add_glyph_to_atlas(
    font_atlases: &mut Vec<FontAtlas>,
    texture_atlases: &mut Assets<TextureAtlasLayout>,
    textures: &mut Assets<Image>,
    font_system: &mut cosmic_text::FontSystem,
    swash_cache: &mut cosmic_text::SwashCache,
    layout_glyph: &cosmic_text::LayoutGlyph,
    font_smoothing: FontSmoothing,
) -> Result<GlyphAtlasInfo, TextError> {
    let physical_glyph = layout_glyph.physical((0., 0.), 1.0);

    let (glyph_texture, offset) =
        get_outlined_glyph_texture(font_system, swash_cache, &physical_glyph, font_smoothing)?;
    let mut add_char_to_font_atlas = |atlas: &mut FontAtlas| -> Result<(), TextError> {
        atlas.add_glyph(
            textures,
            texture_atlases,
            physical_glyph.cache_key,
            &glyph_texture,
            offset,
        )
    };
    if !font_atlases
        .iter_mut()
        .any(|atlas| add_char_to_font_atlas(atlas).is_ok())
    {
        // Find the largest dimension of the glyph, either its width or its height
        let glyph_max_size: u32 = glyph_texture
            .texture_descriptor
            .size
            .height
            .max(glyph_texture.width());
        // Pick the higher of 512 or the smallest power of 2 greater than glyph_max_size
        let containing = (1u32 << (32 - glyph_max_size.leading_zeros())).max(512);

        let mut new_atlas = FontAtlas::new(
            textures,
            texture_atlases,
            UVec2::splat(containing),
            font_smoothing,
        );

        new_atlas.add_glyph(
            textures,
            texture_atlases,
            physical_glyph.cache_key,
            &glyph_texture,
            offset,
        )?;

        font_atlases.push(new_atlas);
    }

    get_glyph_atlas_info(font_atlases, physical_glyph.cache_key)
        .ok_or(TextError::InconsistentAtlasState)
}

/// Rasterises a glyph with RGB subpixel antialiasing via `swash`, bypassing
/// `cosmic_text`'s [`SwashCache`](cosmic_text::SwashCache) (which hardcodes
/// `swash::zeno::Format::Alpha`).
///
/// Mirrors the pattern from `cosmic-text-0.16.0/src/swash.rs:25–78`, but sets
/// `.format(swash::zeno::Format::Subpixel)` to obtain RGB-packed coverage per
/// channel.
///
/// swash's subpixel output is laid out as 4 bytes per pixel
/// `[R_cov, G_cov, B_cov, 0]` (the 4th byte is never written by the
/// rasteriser — see `zeno-0.3.3/src/mask.rs:260–372`). The buffer is therefore
/// already `Rgba8UnormSrgb`-shaped; we just overwrite the alpha byte with 255
/// so the output flows through the existing [`DynamicTextureAtlasBuilder`]
/// path unchanged. The alpha-is-255 convention is intentional: the per-channel
/// alpha lives in the RGB channels and is consumed by the subpixel fragment
/// shader (wired up in phase-03).
///
/// No caching is performed here; repeated calls re-rasterise. A cache on top
/// of this entry point is planned as a phase-04 follow-up.
fn rasterise_subpixel_glyph(
    font_system: &mut cosmic_text::FontSystem,
    physical_glyph: &cosmic_text::PhysicalGlyph,
) -> Result<(Image, IVec2), TextError> {
    use swash::scale::{Render, ScaleContext, Source, StrikeWith};
    use swash::zeno::{Format, Vector};

    let cache_key = physical_glyph.cache_key;

    let font = font_system
        .get_font(cache_key.font_id, cache_key.font_weight)
        .ok_or(TextError::FailedToGetGlyphImage(cache_key))?;

    // Match cosmic_text's variable-font weight handling so `wght`-axis fonts
    // rasterise identically between the grayscale and subpixel paths.
    let variable_width = font
        .as_swash()
        .variations()
        .find_by_tag(swash::Tag::from_be_bytes(*b"wght"));

    let mut context = ScaleContext::new();
    let mut scaler_builder = context
        .builder(font.as_swash())
        .size(f32::from_bits(cache_key.font_size_bits))
        .hint(
            !cache_key
                .flags
                .contains(cosmic_text::CacheKeyFlags::DISABLE_HINTING),
        );
    if let Some(variation) = variable_width {
        scaler_builder = scaler_builder.variations(core::iter::once(swash::Setting {
            tag: swash::Tag::from_be_bytes(*b"wght"),
            value: f32::from(cache_key.font_weight.0)
                .clamp(variation.min_value(), variation.max_value()),
        }));
    }
    let mut scaler = scaler_builder.build();

    // Fractional offset — same quantisation as cosmic_text.
    let offset = if cache_key
        .flags
        .contains(cosmic_text::CacheKeyFlags::PIXEL_FONT)
    {
        Vector::new(
            cache_key.x_bin.as_float().round() + 1.0,
            cache_key.y_bin.as_float().round(),
        )
    } else {
        Vector::new(cache_key.x_bin.as_float(), cache_key.y_bin.as_float())
    };

    let transform = if cache_key
        .flags
        .contains(cosmic_text::CacheKeyFlags::FAKE_ITALIC)
    {
        Some(swash::zeno::Transform::skew(
            swash::zeno::Angle::from_degrees(14.0),
            swash::zeno::Angle::from_degrees(0.0),
        ))
    } else {
        None
    };

    let image = Render::new(&[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    .format(Format::Subpixel)
    .offset(offset)
    .transform(transform)
    .render(&mut scaler, cache_key.glyph_id)
    .ok_or(TextError::FailedToGetGlyphImage(cache_key))?;

    let swash::zeno::Placement {
        left,
        top,
        width,
        height,
    } = image.placement;

    // swash emits `[R_cov, G_cov, B_cov, 0]` per pixel. Overwrite the unused
    // 4th byte with 255 so the final image is a valid `Rgba8UnormSrgb` buffer.
    let mut data = image.data;
    debug_assert_eq!(
        data.len(),
        (width as usize) * (height as usize) * 4,
        "swash subpixel output size mismatch; expected 4 bytes per pixel"
    );
    for pixel in data.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    Ok((
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        ),
        IVec2::new(left, top),
    ))
}

/// Get the texture of the glyph as a rendered image, and its offset
pub fn get_outlined_glyph_texture(
    font_system: &mut cosmic_text::FontSystem,
    swash_cache: &mut cosmic_text::SwashCache,
    physical_glyph: &cosmic_text::PhysicalGlyph,
    font_smoothing: FontSmoothing,
) -> Result<(Image, IVec2), TextError> {
    // Subpixel rasterisation bypasses cosmic_text's `SwashCache` because the
    // latter hardcodes `Format::Alpha` (see `cosmic-text-0.16.0/src/swash.rs:65`).
    // The output of `rasterise_subpixel_glyph` is still an `Rgba8UnormSrgb`
    // image, so it flows into the existing `FontAtlas` unchanged.
    if font_smoothing == FontSmoothing::SubpixelAntiAliased {
        return rasterise_subpixel_glyph(font_system, physical_glyph);
    }

    // NOTE: Ideally, we'd ask COSMIC Text to honor the font smoothing setting directly.
    // However, since it currently doesn't support that, we render the glyph with antialiasing
    // and apply a threshold to the alpha channel to simulate the effect.
    //
    // This has the side effect of making regular vector fonts look quite ugly when font smoothing
    // is turned off, but for fonts that are specifically designed for pixel art, it works well.
    //
    // See: https://github.com/pop-os/cosmic-text/issues/279
    let image = swash_cache
        .get_image_uncached(font_system, physical_glyph.cache_key)
        .ok_or(TextError::FailedToGetGlyphImage(physical_glyph.cache_key))?;

    let cosmic_text::Placement {
        left,
        top,
        width,
        height,
    } = image.placement;

    let data = match image.content {
        cosmic_text::SwashContent::Mask => {
            if font_smoothing == FontSmoothing::None {
                image
                    .data
                    .iter()
                    // Apply a 50% threshold to the alpha channel
                    .flat_map(|a| [255, 255, 255, if *a > 127 { 255 } else { 0 }])
                    .collect()
            } else {
                image
                    .data
                    .iter()
                    .flat_map(|a| [255, 255, 255, *a])
                    .collect()
            }
        }
        cosmic_text::SwashContent::Color => image.data,
        cosmic_text::SwashContent::SubpixelMask => {
            // TODO: implement
            todo!()
        }
    };

    Ok((
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        ),
        IVec2::new(left, top),
    ))
}

/// Generates the [`GlyphAtlasInfo`] for the given subpixel-offset glyph.
pub fn get_glyph_atlas_info(
    font_atlases: &mut [FontAtlas],
    cache_key: cosmic_text::CacheKey,
) -> Option<GlyphAtlasInfo> {
    font_atlases.iter().find_map(|atlas| {
        atlas
            .get_glyph_index(cache_key)
            .map(|location| GlyphAtlasInfo {
                location,
                texture_atlas: atlas.texture_atlas.id(),
                texture: atlas.texture.id(),
            })
    })
}
