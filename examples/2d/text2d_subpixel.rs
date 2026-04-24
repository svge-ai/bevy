//! Side-by-side comparison of the three [`FontSmoothing`] variants at four
//! font sizes — `Text2d` / world-space edition.
//!
//! This is the `Text2d` counterpart to `examples/ui/text_subpixel.rs`. It
//! exercises `bevy_sprite_render`'s subpixel-antialiased text pipeline (added
//! in spec/0002b phase-04) by spawning three columns of `Text2d` entities,
//! one per [`FontSmoothing`] variant, at four realistic body sizes.
//!
//! On an adapter that supports `wgpu::Features::DUAL_SOURCE_BLENDING` (Metal,
//! Vulkan on most modern GPUs, DX12), the `SubpixelAntiAliased` column should
//! look visibly sharper than the `AntiAliased` column at 10pt and 14pt —
//! particularly on the vertical stems of `l`, `i`, `k`, `b`, on the round
//! strokes of digits, and on the small punctuation in the code snippet. On
//! adapters without DSB the subpixel column transparently falls back to
//! grayscale AA (the glyph atlas is still rasterised in subpixel coverage
//! form, but the non-subpixel fragment entry only reads the R channel as
//! alpha).
//!
//! See `examples/ui/text_subpixel.rs` for the UI/overlay version; the
//! rendering should look identical between the two at the same font size.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::text::{FontSmoothing, SubpixelLcdLayout, SubpixelTextSettings};

/// Prose sample — one line of English at four sizes exercises most of the
/// Latin lowercase and caps.
const PROSE: &str = "The quick brown fox jumps over the lazy dog.";

/// Code sample — punctuation and monospace proportions stress subpixel
/// positioning.
const CODE: &str = "fn render(ctx: &mut Context) -> Result<()> { ctx.flush() }";

/// Digits — round strokes and small verticals are where grayscale AA looks
/// worst and subpixel AA helps most.
const DIGITS: &str = "0123456789  3.14159  1,234,567  -42";

/// Four realistic body sizes: small UI label, reading text, heading, large
/// display. Subpixel AA has the most visible impact at 10pt and 14pt.
const SIZES: [f32; 4] = [10.0, 14.0, 20.0, 32.0];

const SMOOTHINGS: [(FontSmoothing, &str); 3] = [
    (FontSmoothing::None, "None"),
    (FontSmoothing::AntiAliased, "AntiAliased"),
    (FontSmoothing::SubpixelAntiAliased, "SubpixelAntiAliased"),
];

// Layout constants in world units. One world unit == one pixel with the
// default 2d camera scaling, so these match physical pixels on a 1:1 display.
const CELL_WIDTH: f32 = 360.0;
const CELL_GAP_X: f32 = 16.0;
const ROW_GAP_Y: f32 = 12.0;
// Within a cell, this much vertical space is reserved per text sample. Wide
// enough for 32pt with extra leading; smaller sizes leave blank space.
const SAMPLE_LINE_HEIGHT: f32 = 40.0;
// 4 samples per cell: size badge + 3 body samples. Padded.
const CELL_HEIGHT: f32 = SAMPLE_LINE_HEIGHT * 4.0 + 24.0;

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins)
        .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.08)))
        .add_systems(Startup, setup);

    // Optional override of `SubpixelTextSettings::enhanced_contrast` for
    // demonstrating the tunable.
    if let Ok(raw) = std::env::var("BEVY_TEXT_SUBPIXEL_ENHANCED_CONTRAST")
        && let Ok(value) = raw.trim().parse::<f32>()
    {
        app.insert_resource(SubpixelTextSettings {
            enhanced_contrast: value,
            ..Default::default()
        });
    }

    // Optional override of `SubpixelLcdLayout`.
    if let Ok(raw) = std::env::var("BEVY_TEXT_SUBPIXEL_LCD_LAYOUT") {
        let normalised = raw.trim().to_ascii_lowercase().replace('_', "-");
        let layout = match normalised.as_str() {
            "horizontal-rgb" | "hrgb" | "rgb" => Some(SubpixelLcdLayout::HorizontalRgb),
            "horizontal-bgr" | "hbgr" | "bgr" => Some(SubpixelLcdLayout::HorizontalBgr),
            "vertical-rgb" | "vrgb" => Some(SubpixelLcdLayout::VerticalRgb),
            "vertical-bgr" | "vbgr" => Some(SubpixelLcdLayout::VerticalBgr),
            _ => None,
        };
        if let Some(layout) = layout {
            app.insert_resource(layout);
        }
    }

    // Optional automated screenshot capture for CI / PR body asset generation.
    if let Ok(path) = std::env::var("BEVY_TEXT_SUBPIXEL_SCREENSHOT") {
        app.insert_resource(ScreenshotPath(path));
        app.insert_resource(ScreenshotFrame(0));
        app.add_systems(Update, take_screenshot_after_warmup);
    }

    app.run();
}

#[derive(Resource)]
struct ScreenshotPath(String);

#[derive(Resource)]
struct ScreenshotFrame(u32);

fn take_screenshot_after_warmup(
    mut commands: Commands,
    path: Res<ScreenshotPath>,
    mut frame: ResMut<ScreenshotFrame>,
    mut exit: MessageWriter<AppExit>,
) {
    frame.0 += 1;
    if frame.0 == 30 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.0.clone()));
    }
    if frame.0 >= 90 {
        exit.write(AppExit::Success);
    }
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(Camera2d);

    let body_font = asset_server.load("fonts/FiraSans-Bold.ttf");
    let mono_font = asset_server.load("fonts/FiraMono-Medium.ttf");

    // Compute the top-left origin of the triptych grid in world space. With
    // `Camera2d`'s default transform, the origin (0, 0) is the center of the
    // screen; positive y is up. We lay out rows top-to-bottom.
    let total_width =
        CELL_WIDTH * SMOOTHINGS.len() as f32 + CELL_GAP_X * (SMOOTHINGS.len() as f32 - 1.0);
    let grid_left = -total_width * 0.5;
    let header_height = 36.0;
    let caption_height = 28.0;
    let total_height = caption_height
        + header_height
        + CELL_HEIGHT * SIZES.len() as f32
        + ROW_GAP_Y * (SIZES.len() as f32 - 1.0);
    let grid_top = total_height * 0.5;

    // Caption across the top of the screen.
    commands.spawn((
        Text2d::new(
            "Text2d FontSmoothing comparison — SubpixelAntiAliased should \
             look sharper at 10pt and 14pt on a DSB-capable adapter.",
        ),
        TextFont {
            font: body_font.clone(),
            font_size: 12.0,
            font_smoothing: FontSmoothing::AntiAliased,
            ..default()
        },
        TextColor(Color::srgb(0.70, 0.70, 0.70)),
        Transform::from_xyz(0.0, grid_top - caption_height * 0.5, 0.0),
    ));

    // Column headers.
    let header_y = grid_top - caption_height - header_height * 0.5;
    for (col, (_, label)) in SMOOTHINGS.iter().enumerate() {
        let cell_center_x = grid_left + CELL_WIDTH * 0.5 + col as f32 * (CELL_WIDTH + CELL_GAP_X);
        commands.spawn((
            Text2d::new(format!("FontSmoothing::{label}")),
            TextFont {
                font: body_font.clone(),
                font_size: 14.0,
                font_smoothing: FontSmoothing::AntiAliased,
                ..default()
            },
            TextColor(Color::srgb(0.55, 0.80, 1.0)),
            Transform::from_xyz(cell_center_x, header_y, 0.0),
        ));
    }

    // Body cells.
    let body_top = header_y - header_height * 0.5;
    for (row, size) in SIZES.iter().enumerate() {
        let cell_top = body_top - row as f32 * (CELL_HEIGHT + ROW_GAP_Y);
        for (col, (smoothing, _)) in SMOOTHINGS.iter().enumerate() {
            let cell_center_x =
                grid_left + CELL_WIDTH * 0.5 + col as f32 * (CELL_WIDTH + CELL_GAP_X);
            let cell_center_y = cell_top - CELL_HEIGHT * 0.5;

            // Cell border (outer rectangle, 1px larger on each side).
            // Colors match the `text_subpixel` UI example's border/background
            // so the two examples read as visually consistent. Both sprites
            // sit at negative z so Text2d entities (default z=0) render on
            // top. Larger z renders on top in bevy_sprite, so z=-0.2 is
            // behind z=-0.1 which is behind z=0.
            commands.spawn((
                Sprite {
                    color: Color::srgb(0.18, 0.18, 0.18),
                    custom_size: Some(Vec2::new(CELL_WIDTH + 2.0, CELL_HEIGHT + 2.0)),
                    ..default()
                },
                Transform::from_xyz(cell_center_x, cell_center_y, -0.2),
            ));
            // Cell background (inner rectangle, exactly cell-sized).
            commands.spawn((
                Sprite {
                    color: Color::srgb(0.04, 0.04, 0.04),
                    custom_size: Some(Vec2::new(CELL_WIDTH, CELL_HEIGHT)),
                    ..default()
                },
                Transform::from_xyz(cell_center_x, cell_center_y, -0.1),
            ));

            // Size badge (top of the cell).
            commands.spawn((
                Text2d::new(format!("{size:>4.1}pt")),
                TextFont {
                    font: body_font.clone(),
                    font_size: 11.0,
                    font_smoothing: FontSmoothing::AntiAliased,
                    ..default()
                },
                TextColor(Color::srgb(0.45, 0.45, 0.45)),
                Transform::from_xyz(cell_center_x, cell_top - SAMPLE_LINE_HEIGHT * 0.5, 0.0),
            ));

            // Prose (sans).
            commands.spawn((
                Text2d::new(PROSE),
                TextFont {
                    font: body_font.clone(),
                    font_size: *size,
                    font_smoothing: *smoothing,
                    ..default()
                },
                TextColor(Color::WHITE),
                Transform::from_xyz(cell_center_x, cell_top - SAMPLE_LINE_HEIGHT * 1.5, 0.0),
            ));

            // Code (mono).
            commands.spawn((
                Text2d::new(CODE),
                TextFont {
                    font: mono_font.clone(),
                    font_size: *size,
                    font_smoothing: *smoothing,
                    ..default()
                },
                TextColor(Color::srgb(0.90, 0.90, 0.78)),
                Transform::from_xyz(cell_center_x, cell_top - SAMPLE_LINE_HEIGHT * 2.5, 0.0),
            ));

            // Digits (mono).
            commands.spawn((
                Text2d::new(DIGITS),
                TextFont {
                    font: mono_font.clone(),
                    font_size: *size,
                    font_smoothing: *smoothing,
                    ..default()
                },
                TextColor(Color::srgb(0.78, 0.88, 1.0)),
                Transform::from_xyz(cell_center_x, cell_top - SAMPLE_LINE_HEIGHT * 3.5, 0.0),
            ));
        }
    }
}
