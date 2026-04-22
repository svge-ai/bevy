//! Shared resources for RGB subpixel antialiased text rendering.
//!
//! These types were originally defined in `bevy_ui_render` (spec/0002 phase-03
//! and spec/0002b phases 01–02). Spec/0002b phase-04 moved them into
//! `bevy_text` so the sprite-render crate (`bevy_sprite_render`, which drives
//! `Text2d`) and the UI-render crate (`bevy_ui_render`) can share a single
//! set of tuning knobs — users no longer have to configure one resource per
//! pipeline.
//!
//! `bevy_ui_render` re-exports the original names for backward compatibility
//! (see `bevy_ui_render::{SubpixelTextSettings, SubpixelLcdLayout,
//! UiSubpixelCapable}`).
//!
//! GPU-facing wiring (uniforms, bind groups, shader code) stays in the render
//! crates — this module only hosts the plain-data resources that both
//! pipelines consume.

use bevy_ecs::resource::Resource;
use bevy_math::Vec4;

/// Tracks whether the active wgpu adapter exposes
/// [`wgpu::Features::DUAL_SOURCE_BLENDING`](https://docs.rs/wgpu/latest/wgpu/struct.Features.html#associatedconstant.DUAL_SOURCE_BLENDING),
/// which [`FontSmoothing::SubpixelAntiAliased`](crate::FontSmoothing::SubpixelAntiAliased)
/// requires for its dual-source-blend shader in both `bevy_ui_render` and
/// `bevy_sprite_render`.
///
/// Inserted once at render startup by `bevy_ui_render::init_ui_subpixel_capability`
/// and `bevy_sprite_render::init_sprite_subpixel_capability`; either system
/// produces the same value (both read the same adapter feature set), so the
/// resource ends up consistent regardless of which render sub-app runs first.
///
/// When `false`, the subpixel queue paths in both renderers transparently fall
/// back to the grayscale pipeline variant — the RGB coverage atlas is still
/// sampled, but the non-subpixel fragment entry only uses the R channel as
/// alpha, so glyphs render as approximate grayscale AA without panicking.
///
/// Renamed from `UiSubpixelCapable` (the type name in spec/0002 phase-03 and
/// spec/0002b phases 01–03) as part of the consolidation in spec/0002b
/// phase-04. `bevy_ui_render` still re-exports the old name.
#[derive(Resource, Debug, Clone, Copy)]
pub struct SubpixelCapable(pub bool);

/// Tuning parameters for RGB subpixel antialiased text rendering.
///
/// Only consulted when [`FontSmoothing::SubpixelAntiAliased`](crate::FontSmoothing::SubpixelAntiAliased)
/// is active and [`SubpixelCapable`] is `true`. Defaults match GPUI's
/// gamma=1.8 preset, which works well across dark and light UI backgrounds.
///
/// Shared between `bevy_ui_render` (UI overlay text) and `bevy_sprite_render`
/// (world-space `Text2d`). Both pipelines read the same resource so app
/// authors only tune subpixel rendering once.
///
/// App authors tuning for a specific display or background can override:
/// - `enhanced_contrast`: higher values yield more aggressive per-channel
///   gamma; lower values are more muted (useful on very low-contrast
///   backgrounds).
/// - `gamma_ratios`: cubic-polynomial coefficients matching GPUI's
///   `GAMMA_INCORRECT_TARGET_RATIOS` table. Alternate rows of that table
///   correspond to different target gammas (1.0, 1.2, ... 2.2).
///
/// ```
/// use bevy_math::Vec4;
/// use bevy_ecs::prelude::*;
/// use bevy_text::SubpixelTextSettings;
///
/// # let mut world = World::new();
/// world.insert_resource(SubpixelTextSettings {
///     enhanced_contrast: 0.35,
///     gamma_ratios: Vec4::new(0.14746, -0.89481, 1.47021, -0.32474),
/// });
/// ```
#[derive(Resource, Debug, Clone, Copy)]
pub struct SubpixelTextSettings {
    /// Strength of the per-channel contrast boost applied before gamma
    /// correction. GPUI's default is `0.5`.
    pub enhanced_contrast: f32,
    /// Cubic-polynomial coefficients used by the subpixel gamma correction.
    /// Defaults match GPUI's gamma=1.8 row of `GAMMA_INCORRECT_TARGET_RATIOS`
    /// scaled by `NORM13`/`NORM24`. See
    /// `references/zed/crates/gpui/src/platform.rs::get_gamma_correction_ratios`
    /// for the source table and the derivation.
    pub gamma_ratios: Vec4,
}

impl Default for SubpixelTextSettings {
    fn default() -> Self {
        Self {
            enhanced_contrast: 0.5,
            gamma_ratios: Vec4::new(0.14746, -0.89481, 1.47021, -0.32474),
        }
    }
}

/// Subpixel arrangement of the target LCD panel.
///
/// Defaults to [`SubpixelLcdLayout::HorizontalRgb`] — the arrangement of
/// ~99% of desktop LCDs and nearly all laptop panels. Override for BGR
/// panels (some older displays) or rotated portrait displays.
///
/// Only consulted when [`FontSmoothing::SubpixelAntiAliased`](crate::FontSmoothing::SubpixelAntiAliased)
/// is active and [`SubpixelCapable`] is `true`. Automatic detection of the
/// host panel's layout is deliberately out of scope — each platform's API is
/// fiddly enough to be its own future spec.
///
/// Shared between `bevy_ui_render` and `bevy_sprite_render`.
///
/// # Limitations of the vertical variants
///
/// The glyph atlas is produced by
/// `bevy_text::font_atlas::rasterise_subpixel_glyph`, which invokes `swash`
/// with [`Format::Subpixel`](https://docs.rs/swash/latest/swash/zeno/enum.Format.html).
/// swash emits three coverage values *per logical pixel*, pre-offset along
/// the horizontal subpixel stripe. The atlas therefore already encodes the
/// R-at-left / G-at-center / B-at-right geometry.
///
/// For [`SubpixelLcdLayout::HorizontalRgb`] the shader samples and emits the
/// atlas RGB as-is. For [`SubpixelLcdLayout::HorizontalBgr`] the shader
/// swizzles to `.bgr`, which inverts the colour-fringe direction — on a
/// physically BGR panel this yields correct subpixel antialiasing.
///
/// The vertical variants ([`SubpixelLcdLayout::VerticalRgb`] /
/// [`SubpixelLcdLayout::VerticalBgr`]) are wired through the same uniform so
/// apps can toggle them, but correct vertical-subpixel antialiasing would
/// require re-rasterising the glyph with a rotated subpixel direction — the
/// current atlas is horizontally pre-offset and cannot be re-used. With this
/// phase they still produce distinct output from `HorizontalRgb` (proof of
/// wiring), but aren't actually correct on a vertical-subpixel panel. A
/// follow-up spec can either rotate the sample pattern at rasterisation
/// time or maintain a second vertical-subpixel atlas.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SubpixelLcdLayout {
    /// Red at left, green centered, blue at right. Default and most common.
    #[default]
    HorizontalRgb,
    /// Blue at left, green centered, red at right. Some older displays.
    HorizontalBgr,
    /// Red at top, green centered, blue at bottom. See type-level note —
    /// requires a rasteriser change to be visually correct; currently acts
    /// as a proof-of-wiring knob only.
    VerticalRgb,
    /// Blue at top, green centered, red at bottom. See type-level note —
    /// requires a rasteriser change to be visually correct.
    VerticalBgr,
}

impl SubpixelLcdLayout {
    /// Matches the discriminants consumed by the subpixel fragment entries in
    /// both `bevy_ui_render`'s `ui.wgsl` and `bevy_sprite_render`'s
    /// `sprite.wgsl`. Keep the numeric values in sync with the `LAYOUT_*`
    /// constants declared in those shaders.
    pub fn shader_flags(self) -> u32 {
        match self {
            SubpixelLcdLayout::HorizontalRgb => 0,
            SubpixelLcdLayout::HorizontalBgr => 1,
            SubpixelLcdLayout::VerticalRgb => 2,
            SubpixelLcdLayout::VerticalBgr => 3,
        }
    }
}
