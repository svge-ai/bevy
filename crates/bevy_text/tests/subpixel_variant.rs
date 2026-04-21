//! Smoke test for the `FontSmoothing::SubpixelAntiAliased` variant introduced in
//! spec 0002 / phase-01. Verifies the enum is constructible and settable via the
//! `TextFont::with_font_smoothing` builder.

use bevy_text::{FontSmoothing, TextFont};

#[test]
fn subpixel_variant_is_constructible() {
    let font = TextFont::default().with_font_smoothing(FontSmoothing::SubpixelAntiAliased);
    assert_eq!(font.font_smoothing, FontSmoothing::SubpixelAntiAliased);
}
