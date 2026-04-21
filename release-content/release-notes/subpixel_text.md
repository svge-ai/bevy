---
title: RGB Subpixel Antialiased Text
authors: ["@masegraye"]
pull_requests: []
---

Bevy can now render text with **RGB subpixel antialiasing**, the technique
behind the visibly crisper text you see in Zed, macOS native apps, and most
modern code editors. Opt in per-`Text` by setting
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

Subpixel AA requires `wgpu::Features::DUAL_SOURCE_BLENDING`, which is
available on Metal, DX12, and Vulkan on most modern GPUs, but not on WebGL2
or some older mobile adapters. Bevy requests the feature as optional; on
adapters where it is unavailable, `SubpixelAntiAliased` transparently falls
back to `AntiAliased` so your app continues to render correctly. Detect the
active state at runtime via the `UiSubpixelCapable` resource.

This PR wires up the `bevy_ui` text path. The `Text2d` (world-space sprite)
path is planned as a follow-up.

See the `text_subpixel` example for a side-by-side comparison of the three
`FontSmoothing` variants at multiple font sizes.
