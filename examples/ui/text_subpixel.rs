//! Visual comparison of the three [`FontSmoothing`] variants at three small
//! font sizes.
//!
//! Use this example to eyeball the quality difference between
//! [`FontSmoothing::AntiAliased`] (Bevy's default grayscale AA) and
//! [`FontSmoothing::SubpixelAntiAliased`] (the RGB subpixel path added in
//! spec 0002). [`FontSmoothing::None`] is included as a reference point for
//! "no smoothing".
//!
//! On an adapter that supports `wgpu::Features::DUAL_SOURCE_BLENDING` (Metal,
//! Vulkan on most modern GPUs, DX12), the subpixel row should look visibly
//! sharper than the anti-aliased row at 10pt and 14pt — particularly on the
//! vertical stems of `l`, `i`, `k`, `b`. On adapters without DSB, the subpixel
//! row transparently falls back to grayscale AA (see [`UiSubpixelCapable`]
//! in `bevy_ui_render`).

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::text::FontSmoothing;

const PANGRAM: &str = "The quick brown fox jumps over the lazy dog";

const SIZES: [f32; 3] = [10.0, 14.0, 20.0];
const SMOOTHINGS: [(FontSmoothing, &str); 3] = [
    (FontSmoothing::None, "None"),
    (FontSmoothing::AntiAliased, "AntiAliased"),
    (FontSmoothing::SubpixelAntiAliased, "SubpixelAntiAliased"),
];

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins)
        .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.08)))
        .add_systems(Startup, setup);

    // Optional automated screenshot for phase-03 exit-criterion capture. Set
    // `BEVY_TEXT_SUBPIXEL_SCREENSHOT=<path>` to have the example grab the
    // primary window a few frames after startup and write a PNG to that path.
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
    // Capture on frame 30 (≈0.5s at 60fps) so the atlas has warmed up.
    if frame.0 == 30 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.0.clone()));
    }
    // Exit a handful of frames later so `save_to_disk` has landed on disk.
    if frame.0 >= 90 {
        exit.write(AppExit::Success);
    }
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(Camera2d);

    let font = asset_server.load("fonts/FiraMono-Medium.ttf");

    commands
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(24)),
            row_gap: px(18),
            ..default()
        })
        .with_children(|parent| {
            for (smoothing, label) in SMOOTHINGS {
                parent
                    .spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(4),
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            Text::new(format!("FontSmoothing::{label}")),
                            TextFont {
                                font: font.clone(),
                                font_size: 13.0,
                                font_smoothing: FontSmoothing::AntiAliased,
                                ..default()
                            },
                            TextColor(Color::srgb(0.55, 0.80, 1.0)),
                        ));

                        for size in SIZES {
                            row.spawn((
                                Text::new(format!("{size:>4.1}pt  {PANGRAM}")),
                                TextFont {
                                    font: font.clone(),
                                    font_size: size,
                                    font_smoothing: smoothing,
                                    ..default()
                                },
                                TextColor(Color::WHITE),
                            ));
                        }
                    });
            }
        });
}
