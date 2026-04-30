use alloc::borrow::Cow;

use core::hash::BuildHasher;

use bevy_asset::{AssetId, Assets, Handle};
use bevy_color::Color;
use bevy_ecs::{
    component::Component, entity::Entity, reflect::ReflectComponent, resource::Resource,
    system::ResMut,
};
use bevy_image::prelude::*;
use bevy_log::warn_once;
use bevy_math::{Rect, Vec2};
use bevy_platform::collections::HashSet;
use bevy_platform::hash::FixedHasher;
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use parley::style::{OverflowWrap, TextWrapMode, WordBreak};
use parley::{
    Alignment, AlignmentOptions, FontFamily, Layout, PositionedLayoutItem, StyleProperty,
};
use swash::FontRef;

use crate::TextBrush;
use crate::{
    add_glyph_to_atlas,
    error::TextError,
    get_glyph_atlas_info,
    parley_context::{FontCx, LayoutCx, ScaleCx},
    ComputedTextBlock, Font, FontAtlasKey, FontAtlasSet, FontHinting, FontSmoothing, FontSource,
    Justify, LetterSpacing, LineBreak, LineHeight, PositionedGlyph, SubpixelBucket, TextBounds,
    TextEntity, TextFont, TextLayout,
};

struct TextSectionView<'a> {
    index: usize,
    text: &'a str,
    text_font: &'a TextFont,
    font_size: f32,
    line_height: LineHeight,
    letter_spacing: LetterSpacing,
}

/// The `TextPipeline` is used to layout and render text blocks (see `Text`/`Text2d`).
#[derive(Resource, Default)]
pub struct TextPipeline {
    /// Buffered vec for collecting text sections.
    ///
    /// See <https://users.rust-lang.org/t/how-to-cache-a-vectors-capacity/94478/10>.
    sections_buffer: Vec<TextSectionView<'static>>,
    /// Buffered string for concatenated text content.
    text_buffer: String,
}

impl TextPipeline {
    /// Shapes and lays out text spans into the computed buffer.
    ///
    /// Negative or 0.0 font sizes will not be laid out.
    pub fn update_buffer<'a>(
        &mut self,
        fonts: &Assets<Font>,
        text_spans: impl Iterator<
            Item = (
                Entity,
                usize,
                &'a str,
                &'a TextFont,
                Color,
                LineHeight,
                LetterSpacing,
            ),
        >,
        linebreak: LineBreak,
        justify: Justify,
        bounds: TextBounds,
        scale_factor: f32,
        computed: &mut ComputedTextBlock,
        font_system: &mut FontCx,
        layout_cx: &mut LayoutCx,
        logical_viewport_size: Vec2,
        base_rem_size: f32,
    ) -> Result<(), TextError> {
        computed.entities.clear();
        computed.needs_rerender = false;
        computed.uses_rem_sizes = false;
        computed.uses_viewport_sizes = false;

        if scale_factor <= 0.0 {
            warn_once!("Text scale factor is <= 0.0. No text will be displayed.");
            return Err(TextError::DegenerateScaleFactor);
        }

        let mut sections: Vec<TextSectionView<'_>> = core::mem::take(&mut self.sections_buffer)
            .into_iter()
            .map(|_| -> TextSectionView<'_> { unreachable!() })
            .collect();

        let result = {
            for (index, (entity, depth, text, text_font, _color, line_height, letter_spacing)) in
                text_spans.enumerate()
            {
                match text_font.font_size {
                    crate::FontSize::Vw(_)
                    | crate::FontSize::Vh(_)
                    | crate::FontSize::VMin(_)
                    | crate::FontSize::VMax(_) => computed.uses_viewport_sizes = true,
                    crate::FontSize::Rem(_) => computed.uses_rem_sizes = true,
                    _ => (),
                }

                computed.entities.push(TextEntity {
                    entity,
                    depth,
                    font_smoothing: text_font.font_smoothing,
                });

                if text.is_empty() {
                    continue;
                }

                if matches!(text_font.font, FontSource::Handle(_))
                    && resolve_font_source(&text_font.font, fonts).is_err()
                {
                    return Err(TextError::NoSuchFont);
                }

                let font_size = text_font
                    .font_size
                    .eval(logical_viewport_size, base_rem_size);

                if font_size <= 0.0 {
                    warn_once!(
                        "Text span {entity} has a font size <= 0.0. Nothing will be displayed."
                    );
                    continue;
                }

                const WARN_FONT_SIZE: f32 = 1000.0;
                if font_size > WARN_FONT_SIZE {
                    warn_once!(
                        "Text span {entity} has an excessively large font size ({} with scale factor {}). \
                        Extremely large font sizes will cause performance issues with font atlas \
                        generation and high memory usage.",
                        font_size,
                        scale_factor,
                    );
                }

                sections.push(TextSectionView {
                    index,
                    text,
                    text_font,
                    font_size,
                    line_height,
                    letter_spacing,
                });
            }

            self.text_buffer.clear();
            for section in &sections {
                self.text_buffer.push_str(section.text);
            }

            let text = self.text_buffer.as_str();
            let layout = &mut computed.layout;
            let mut builder =
                layout_cx
                    .0
                    .ranged_builder(&mut font_system.0, text, scale_factor, true);

            match linebreak {
                LineBreak::AnyCharacter => {
                    builder.push_default(StyleProperty::WordBreak(WordBreak::BreakAll));
                }
                LineBreak::WordOrCharacter => {
                    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
                }
                LineBreak::NoWrap => {
                    builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
                }
                LineBreak::WordBoundary => {
                    builder.push_default(StyleProperty::WordBreak(WordBreak::Normal));
                }
            }

            let mut start = 0;
            for section in sections.drain(..) {
                let end = start + section.text.len();
                let range = start..end;
                start = end;

                if range.is_empty() {
                    continue;
                }

                let family = resolve_font_source(&section.text_font.font, fonts)?;

                builder.push(StyleProperty::FontFamily(family), range.clone());
                builder.push(
                    StyleProperty::Brush(TextBrush::new(
                        section.index as u32,
                        section.text_font.font_smoothing,
                    )),
                    range.clone(),
                );
                builder.push(StyleProperty::FontSize(section.font_size), range.clone());
                builder.push(
                    StyleProperty::LineHeight(section.line_height.eval()),
                    range.clone(),
                );
                builder.push(
                    StyleProperty::LetterSpacing(section.letter_spacing.eval(base_rem_size)),
                    range.clone(),
                );
                builder.push(
                    StyleProperty::FontWeight(section.text_font.weight.into()),
                    range.clone(),
                );
                builder.push(
                    StyleProperty::FontWidth(section.text_font.width.into()),
                    range.clone(),
                );
                builder.push(
                    StyleProperty::FontStyle(section.text_font.style.into()),
                    range.clone(),
                );
                builder.push(
                    StyleProperty::FontFeatures((&section.text_font.font_features).into()),
                    range,
                );
            }

            builder.build_into(layout, text);
            layout_with_bounds(layout, bounds, justify);
            Ok(())
        };

        sections.clear();
        self.sections_buffer = sections
            .into_iter()
            .map(|_| -> TextSectionView<'static> { unreachable!() })
            .collect();

        result
    }

    /// Queues text for measurement.
    pub fn create_text_measure<'a>(
        &mut self,
        entity: Entity,
        fonts: &Assets<Font>,
        text_spans: impl Iterator<
            Item = (
                Entity,
                usize,
                &'a str,
                &'a TextFont,
                Color,
                LineHeight,
                LetterSpacing,
            ),
        >,
        scale_factor: f32,
        layout: &TextLayout,
        computed: &mut ComputedTextBlock,
        font_system: &mut FontCx,
        layout_cx: &mut LayoutCx,
        logical_viewport_size: Vec2,
        base_rem_size: f32,
    ) -> Result<TextMeasureInfo, TextError> {
        const MIN_WIDTH_CONTENT_BOUNDS: TextBounds = TextBounds::new_horizontal(0.0);

        computed.needs_rerender = false;

        self.update_buffer(
            fonts,
            text_spans,
            layout.linebreak,
            layout.justify,
            MIN_WIDTH_CONTENT_BOUNDS,
            scale_factor,
            computed,
            font_system,
            layout_cx,
            logical_viewport_size,
            base_rem_size,
        )?;

        let layout_buffer = &mut computed.layout;
        let min_width_content_size = buffer_dimensions(layout_buffer);

        layout_with_bounds(layout_buffer, TextBounds::UNBOUNDED, layout.justify);
        let max_width_content_size = buffer_dimensions(layout_buffer);

        Ok(TextMeasureInfo {
            min: min_width_content_size,
            max: max_width_content_size,
            entity,
        })
    }

    /// Update [`TextLayoutInfo`] with the new [`PositionedGlyph`] layout.
    ///
    /// `current_frame` is forwarded to [`FontAtlasSet::touch`] so the LRU
    /// eviction tracking (svge-main fork) sees this frame as the last
    /// access for every `FontAtlasKey` exercised here. Pass
    /// `frame_count.0 as u64` from a system that has `Res<FrameCount>`,
    /// or `0` if eviction tracking is not desired.
    pub fn update_text_layout_info(
        &mut self,
        layout_info: &mut TextLayoutInfo,
        font_atlas_set: &mut FontAtlasSet,
        textures: &mut Assets<Image>,
        computed: &mut ComputedTextBlock,
        scale_cx: &mut ScaleCx,
        bounds: TextBounds,
        justify: Justify,
        hinting: FontHinting,
        current_frame: u64,
    ) -> Result<(), TextError> {
        computed.needs_rerender = false;
        layout_info.clear();

        // svge-main fork (LS-gxrtooro): track atlas AssetIds referenced by
        // this layout so we can populate `layout_info.atlas_handles` with
        // deduplicated strong handles at the end. This keeps the
        // underlying atlas `Image` assets alive for the lifetime of this
        // `TextLayoutInfo`, which in turn keeps the asset present in
        // `Assets<Image>` and prevents the weakref-trigger eviction sweep
        // (`FontAtlasSet::evict_stale`, which checks
        // `images.contains(atlas.texture)`) from reaping an atlas the
        // rendering pipeline still needs.
        let mut referenced_atlas_ids: HashSet<AssetId<Image>> = HashSet::default();

        let layout = &mut computed.layout;
        layout_with_bounds(layout, bounds, justify);

        for (line_index, line) in layout.lines().enumerate() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    let section_index = glyph_run.style().brush.section_index as usize;
                    let font_smoothing = glyph_run.style().brush.font_smoothing;
                    let run = glyph_run.run();
                    let font = run.font();
                    let font_size = run.font_size();
                    let coords = run.normalized_coords();
                    let variations_hash = FixedHasher.hash_one(coords);
                    let font_atlas_key = FontAtlasKey {
                        id: font.data.id() as u32,
                        index: font.index,
                        font_size_bits: font_size.to_bits(),
                        variations_hash,
                        hinting,
                        font_smoothing,
                    };

                    // svge-main: mark this key as touched on the current
                    // frame so the LRU eviction in `FontAtlasSet::evict_stale`
                    // doesn't drop atlases for actively-rendered text.
                    font_atlas_set.touch(font_atlas_key, current_frame);

                    let Some(font_ref) =
                        FontRef::from_index(font.data.as_ref(), font.index as usize)
                    else {
                        return Err(TextError::NoSuchFont);
                    };

                    let hint = hinting.is_enabled() && font_smoothing != FontSmoothing::None;
                    let mut scaler = scale_cx
                        .0
                        .builder(font_ref)
                        .size(font_size)
                        .hint(hint)
                        .normalized_coords(coords)
                        .build();

                    for glyph in glyph_run.positioned_glyphs() {
                        let Ok(glyph_id) = u16::try_from(glyph.id) else {
                            continue;
                        };

                        let subpixel_bucket = SubpixelBucket::from_fract(glyph.x, font_smoothing);
                        let subpixel_offset = Vec2::new(subpixel_bucket.rasterise_offset_x(), 0.0);
                        let cache_key = crate::GlyphCacheKey {
                            glyph_id,
                            subpixel_bucket,
                        };

                        let font_atlases = font_atlas_set.entry(font_atlas_key).or_default();
                        // svge-main fork (LS-gxrtooro): `add_glyph_to_atlas`
                        // now returns `(GlyphAtlasInfo, Option<Handle<Image>>)`.
                        // The `Option<Handle>` is populated only on the
                        // new-atlas-allocation path; cache hits and slot-fills
                        // into existing atlases return `None`. Capture the
                        // handle into `referenced_atlas_ids` via `layout_info`
                        // promotion below.
                        let atlas_info = if let Some(info) =
                            get_glyph_atlas_info(font_atlases, cache_key)
                        {
                            info
                        } else {
                            let (info, new_handle) = add_glyph_to_atlas(
                                font_atlases,
                                textures,
                                &mut scaler,
                                font_smoothing,
                                glyph_id,
                                subpixel_bucket,
                                subpixel_offset,
                            )?;
                            // Push the strong handle eagerly — the AssetId
                            // is also captured in `referenced_atlas_ids`
                            // below, but we can't rely on `get_strong_handle`
                            // alone because the asset GC drop event for the
                            // *previous* shape's `atlas_handles` may have
                            // been queued earlier this frame.
                            if let Some(handle) = new_handle {
                                layout_info.atlas_handles.push(handle);
                            }
                            info
                        };

                        // svge-main fork: capture the atlas AssetId for this
                        // glyph so we can promote it to a strong handle on
                        // `layout_info.atlas_handles` after the glyph loop.
                        // Dedup happens via the HashSet.
                        referenced_atlas_ids.insert(atlas_info.texture);

                        let glyph_pos = Vec2::new(glyph.x, glyph.y);
                        let size = atlas_info.rect.size();

                        layout_info.glyphs.push(PositionedGlyph {
                            position: size / 2.
                                + if font_smoothing == FontSmoothing::None {
                                    glyph_pos.floor()
                                } else {
                                    glyph_pos
                                }
                                + atlas_info.offset,
                            atlas_info,
                            section_index,
                            line_index,
                        });
                    }

                    layout_info.run_geometry.push(RunGeometry {
                        section_index,
                        bounds: Rect::new(
                            line.metrics().inline_min_coord + glyph_run.offset(),
                            line.metrics().block_min_coord,
                            line.metrics().inline_min_coord
                                + glyph_run.offset()
                                + glyph_run.advance(),
                            line.metrics().block_max_coord,
                        ),
                        strikethrough_y: glyph_run.baseline() - run.metrics().strikethrough_offset,
                        strikethrough_thickness: run.metrics().strikethrough_size,
                        underline_y: glyph_run.baseline() - run.metrics().underline_offset,
                        underline_thickness: run.metrics().underline_size,
                    });
                }
            }
        }

        layout_info.size = Vec2::new(layout.full_width(), layout.height()).ceil();

        // svge-main fork (LS-gxrtooro): promote each referenced atlas
        // AssetId to a strong `Handle<Image>` and store it on the layout.
        // These strong handles
        // (a) keep the atlas `Image` asset alive across frames so the
        //     renderer can sample it, and
        // (b) drive weakref-trigger eviction in `FontAtlasSet::evict_stale`:
        //     when no `TextLayoutInfo` references an atlas any more, the
        //     atlas's image asset becomes orphaned, the asset GC reaps it,
        //     and the next eviction sweep notices the missing image
        //     (via `images.contains`) and drops the now-stale `FontAtlas`.
        //
        // Newly-allocated atlas handles were already pushed during the
        // glyph loop (the strong handle returned from
        // `add_glyph_to_atlas`); only cache-hit AssetIds need promotion
        // here. Dedup by skipping AssetIds whose handle was already
        // pushed — same-frame allocation handles will match the cache-hit
        // AssetId via `Handle::id()`.
        let already_pushed: HashSet<AssetId<Image>> = layout_info
            .atlas_handles
            .iter()
            .map(|h| h.id())
            .collect();
        for atlas_id in referenced_atlas_ids {
            if already_pushed.contains(&atlas_id) {
                continue;
            }
            // `get_strong_handle` returns `None` for assets not in the
            // collection (e.g. UUID-keyed assets, or assets already
            // dropped). Skip — wouldn't have rendered anyway.
            if let Some(handle) = textures.get_strong_handle(atlas_id) {
                layout_info.atlas_handles.push(handle);
            }
        }

        Ok(())
    }
}

/// Resolve a [`FontSource`], producing a [`FontFamily`], by looking it up in the [`Assets<Font>`] collection.
pub fn resolve_font_source<'a>(
    font: &'a FontSource,
    fonts: &Assets<Font>,
) -> Result<FontFamily<'a>, TextError> {
    Ok(match font {
        FontSource::Handle(handle) => {
            let font = fonts.get(handle.id()).ok_or(TextError::NoSuchFont)?;
            FontFamily::Single(parley::FontFamilyName::Named(Cow::Owned(
                font.family_name.as_str().to_owned(),
            )))
        }
        FontSource::Family(family) => FontFamily::named(family.as_str()),
        generic => {
            #[cfg(not(feature = "system_font_discovery"))]
            bevy_log::error_once!(
                "A generic FontSource ({generic:?}) was used, but the `system_font_discovery` \
                feature is not enabled. Text may not render. Enable the feature to allow Bevy \
                to discover system fonts."
            );
            match generic {
                FontSource::Handle(_) | FontSource::Family(_) => unreachable!(),
                FontSource::Serif => parley::GenericFamily::Serif.into(),
                FontSource::SansSerif => parley::GenericFamily::SansSerif.into(),
                FontSource::Cursive => parley::GenericFamily::Cursive.into(),
                FontSource::Fantasy => parley::GenericFamily::Fantasy.into(),
                FontSource::Monospace => parley::GenericFamily::Monospace.into(),
                FontSource::SystemUi => parley::GenericFamily::SystemUi.into(),
                FontSource::UiSerif => parley::GenericFamily::UiSerif.into(),
                FontSource::UiSansSerif => parley::GenericFamily::UiSansSerif.into(),
                FontSource::UiMonospace => parley::GenericFamily::UiMonospace.into(),
                FontSource::UiRounded => parley::GenericFamily::UiRounded.into(),
                FontSource::Emoji => parley::GenericFamily::Emoji.into(),
                FontSource::Math => parley::GenericFamily::Math.into(),
                FontSource::FangSong => parley::GenericFamily::FangSong.into(),
            }
        }
    })
}

/// Render information for a corresponding text block.
///
/// Contains scaled glyphs and their size. Generated via [`TextPipeline::update_text_layout_info`] when an entity has
/// [`TextLayout`] and [`ComputedTextBlock`] components.
#[derive(Component, Clone, Default, Debug, Reflect)]
#[reflect(Component, Default, Debug, Clone)]
pub struct TextLayoutInfo {
    /// The target scale factor for this text layout
    pub scale_factor: f32,
    /// Scaled and positioned glyphs in screenspace
    pub glyphs: Vec<PositionedGlyph>,
    /// Geometry of each text run used to render text decorations like background colors, strikethrough, and underline.
    /// A run in `bevy_text` is a contiguous sequence of glyphs on a line that share the same text attributes like font,
    /// font size, and line height. A text entity that extends over multiple lines will have multiple corresponding runs.
    ///
    /// The coordinates are unscaled and relative to the top left corner of the text layout.
    pub run_geometry: Vec<RunGeometry>,
    /// The glyphs resulting size
    pub size: Vec2,
    /// Cursor size and position for editing
    pub cursor: Option<Rect>,
    /// Selection rects
    pub selection_rects: Vec<Rect>,
    /// Underline rects for the active IME preedit/compose region.
    /// Should only have values when composition is in progress.
    pub preedit_underline_rects: Vec<Rect>,
    /// svge-main fork (LS-gxrtooro): strong [`Handle<Image>`] per font
    /// atlas referenced by the glyphs in this layout.
    ///
    /// Pre-fix, [`crate::FontAtlas`] held a strong `Handle<Image>` itself,
    /// and eviction was time-based: shape gating skipped `touch()` on idle
    /// frames → atlases aged past the idle threshold → evicted while still
    /// in use → text vanished after 2-3 seconds.
    ///
    /// Post-fix, [`crate::FontAtlas`] holds only an [`AssetId<Image>`]
    /// (weak), and the strong handles for active atlases live HERE,
    /// per-`TextLayoutInfo`. When a text entity despawns or re-shapes
    /// with different glyph runs, this `Vec` is replaced; the obsolete
    /// strong handles drop; if no other layout references the same
    /// atlases, the asset GC reaps the underlying `Image` at end-of-frame;
    /// the next [`crate::FontAtlasSet::evict_stale`] sweep notices via
    /// `Assets::contains` and drops the now-stale [`crate::FontAtlas`].
    /// No time threshold, no per-frame `touch()` walk required.
    ///
    /// `update_text_layout_info` populates this field; consumers do not
    /// write to it directly.
    pub atlas_handles: Vec<Handle<Image>>,
}

impl TextLayoutInfo {
    /// Clear the layout, retaining capacity
    pub fn clear(&mut self) {
        self.scale_factor = 1.;
        self.glyphs.clear();
        self.run_geometry.clear();
        self.size = Vec2::ZERO;
        self.cursor = None;
        self.selection_rects.clear();
        self.preedit_underline_rects.clear();
        // svge-main fork: dropping the strong handles here decreases each
        // atlas's strong count. The next eviction sweep will reap any atlas
        // that drops to count == 1 (only the atlas itself owns its handle).
        self.atlas_handles.clear();
    }
}

/// Geometry of a text run used to render text decorations like background colors, strikethrough, and underline.
/// A run in `bevy_text` is a contiguous sequence of glyphs on a line that share the same text attributes like font,
/// font size, and line height.
#[derive(Default, Debug, Clone, Reflect)]
pub struct RunGeometry {
    /// The index of the text entity in [`ComputedTextBlock`] that this run belongs to.
    pub section_index: usize,
    /// Bounding box around the text run.
    pub bounds: Rect,
    /// Y position of the strikethrough in the text layout.
    pub strikethrough_y: f32,
    /// Strikethrough stroke thickness.
    pub strikethrough_thickness: f32,
    /// Y position of the underline in the text layout.
    pub underline_y: f32,
    /// Underline stroke thickness.
    pub underline_thickness: f32,
}

impl RunGeometry {
    /// Returns the center of the strikethrough in the text layout.
    pub fn strikethrough_position(&self) -> Vec2 {
        Vec2::new(
            self.bounds.center().x,
            self.strikethrough_y + 0.5 * self.strikethrough_thickness,
        )
    }

    /// Returns the size of the strikethrough.
    pub fn strikethrough_size(&self) -> Vec2 {
        Vec2::new(self.bounds.size().x, self.strikethrough_thickness)
    }

    /// Returns the center of the underline in the text layout.
    pub fn underline_position(&self) -> Vec2 {
        Vec2::new(
            self.bounds.center().x,
            self.underline_y + 0.5 * self.underline_thickness,
        )
    }

    /// Returns the size of the underline.
    pub fn underline_size(&self) -> Vec2 {
        Vec2::new(self.bounds.size().x, self.underline_thickness)
    }
}

/// Size information for a corresponding [`ComputedTextBlock`] component.
///
/// Generated via [`TextPipeline::create_text_measure`].
#[derive(Debug)]
pub struct TextMeasureInfo {
    /// Minimum size for a text area in pixels, to be used when laying out widgets with taffy.
    pub min: Vec2,
    /// Maximum size for a text area in pixels, to be used when laying out widgets with taffy.
    pub max: Vec2,
    /// The entity that is measured.
    pub entity: Entity,
}

impl TextMeasureInfo {
    /// Computes the size of the text area within the provided bounds.
    pub fn compute_size(
        &mut self,
        bounds: TextBounds,
        computed: &mut ComputedTextBlock,
        _font_system: &mut FontCx,
    ) -> Vec2 {
        // Note that this arbitrarily adjusts the buffer layout. We assume the buffer is always 'refreshed'
        // whenever a canonical state is required.
        let layout = &mut computed.layout;
        layout.break_all_lines(bounds.width);
        layout.align(Alignment::Start, AlignmentOptions::default());
        buffer_dimensions(layout)
    }
}

fn layout_with_bounds(layout: &mut Layout<TextBrush>, bounds: TextBounds, justify: Justify) {
    layout.break_all_lines(bounds.width);
    layout.align(justify.into(), AlignmentOptions::default());
}

/// Calculate the size of the text area for the given buffer.
fn buffer_dimensions(buffer: &Layout<TextBrush>) -> Vec2 {
    let size = Vec2::new(buffer.full_width(), buffer.height());
    if size.is_finite() {
        size.ceil()
    } else {
        Vec2::ZERO
    }
}

/// Discards stale data cached in the font system.
pub(crate) fn trim_source_cache(mut font_cx: ResMut<FontCx>) {
    // A trim age of 2 was found to reduce frame time variance vs age of 1 when tested with dynamic text.
    // See https://github.com/bevyengine/bevy/pull/15037
    //
    // We assume only text updated frequently benefits from the shape cache (e.g. animated text, or
    // text that is dynamically measured for UI).
    font_cx.0.source_cache.prune(2, false);
}
