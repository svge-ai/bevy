---
title: RGB Subpixel Antialiased Text
authors: ["@masegraye"]
pull_requests: []
---

Bevy can now render text with **RGB subpixel antialiasing**, the technique
behind the visibly crisper text you see in Zed, macOS native apps, and most
modern code editors. Opt in per-`Text` (or `Text2d`) by setting
`TextFont::font_smoothing = FontSmoothing::SubpixelAntiAliased`.

Bevy's existing grayscale AA (`FontSmoothing::AntiAliased`, the default) has
an effective horizontal resolution of one physical pixel — a glyph stem that
falls between pixels gets softened into a two-pixel-wide gray blur. Subpixel
AA treats each pixel as three horizontal colour sub-pixels (R, G, B, as
physically arranged on most LCD/OLED panels) and addresses them
independently, roughly tripling the effective horizontal resolution. At 10pt
and 14pt body text the difference is clearly visible: sharper vertical stems,
better digit legibility, no "dancing" between pixel rows. The tradeoff is a
subtle rainbow halo on contrast edges, which is why it is opt-in rather than
the default.

## How to enable

```rust
use bevy::prelude::*;
use bevy::text::FontSmoothing;

commands.spawn((
    Text::new("Sharper body text"),
    TextFont {
        font: asset_server.load("fonts/FiraSans-Bold.ttf"),
        font_size: 14.0,
        font_smoothing: FontSmoothing::SubpixelAntiAliased,
        ..default()
    },
));
```

The same `FontSmoothing` variant works for `Text2d` (world-space sprite
text) without any extra setup — both the UI and sprite render pipelines
share the glyph atlas and the RGB coverage path.

## What's in the feature

- **UI text + `Text2d` support.** Both `bevy_ui_render` and
  `bevy_sprite_render` gain a dual-source-blend subpixel pipeline variant.
  Use the same `FontSmoothing::SubpixelAntiAliased` value in either crate.
- **`SubpixelTextSettings` resource** for app-level tuning. Two fields —
  `enhanced_contrast: f32` (default `0.5`) and `gamma_ratios: Vec4` (the
  gamma=1.8 row of GPUI's `GAMMA_INCORRECT_TARGET_RATIOS`). Apps targeting
  specific display/background/foreground combinations (dark IDE, light
  reading app) can tune these once and both pipelines pick up the change.
- **`SubpixelLcdLayout` resource** for panel orientation. Horizontal RGB is
  the default; `HorizontalBgr`, `VerticalRgb`, and `VerticalBgr` are wired
  through a shader swizzle for BGR panels and rotated displays. Vertical
  variants are proof-of-wiring today — correct vertical-subpixel
  antialiasing needs a rasteriser-rotation follow-up; see the type's
  rustdoc.
- **Per-subpixel-bucket atlas partitioning** via `SubpixelBucket`. Glyphs
  rasterised at different horizontal sub-pixel offsets (four bins, matching
  `cosmic_text::SubpixelBin`) land in separate atlases keyed through the
  restructured `FontAtlasKey`. Non-subpixel smoothing modes use
  `SubpixelBucket::NotApplicable` and retain the prior atlas layout.
- **Graceful DSB-unavailable fallback.** `wgpu::Features::DUAL_SOURCE_BLENDING`
  is requested as an optional adapter feature. On adapters where it's
  unavailable (WebGL2, some older mobile Vulkan, DX11), the
  `SubpixelCapable` resource is `false` and `SubpixelAntiAliased` requests
  transparently downgrade to the grayscale pipeline so apps continue to
  render correctly.
- **Direct-swash rasterisation.** `bevy_text` bypasses
  `cosmic_text::SwashCache`'s hardcoded `Format::Alpha` and calls
  `swash::scale::Render` with `Format::Subpixel` directly, producing RGB
  coverage atlases. When upstream `cosmic_text` exposes `Format` as a
  parameter, this path can collapse to a three-line call.

Detect DSB support at runtime via the `bevy_text::SubpixelCapable`
resource (renamed from `bevy_ui_render::UiSubpixelCapable`, which remains
as a `#[deprecated]` alias so existing code keeps compiling).

See the `text_subpixel` and `text2d_subpixel` examples for side-by-side
comparisons of the three `FontSmoothing` variants at multiple font sizes.
The UI example supports interactive keyboard controls for
`enhanced_contrast` (`1`/`2`/`3`), `SubpixelLcdLayout` (`R`/`B`/`V`/`G`),
and ad-hoc screenshot capture (`S`).
