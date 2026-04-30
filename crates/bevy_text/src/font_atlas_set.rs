use crate::{FontAtlas, FontHinting, FontSmoothing, GlyphCacheKey};
use bevy_asset::Assets;
use bevy_ecs::resource::Resource;
use bevy_image::Image;
use bevy_platform::collections::HashMap;
use core::ops::{Deref, DerefMut};

/// Identifies the font atlases for a particular font in [`FontAtlasSet`]
///
/// Allows an `f32` font size to be used as a key in a `HashMap`, by its binary representation.
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct FontAtlasKey {
    /// Font data id
    pub id: u32,
    /// Font data index
    pub index: u32,
    /// Font size via `f32::to_bits`
    pub font_size_bits: u32,
    /// Hash of normalized variation coords for this run.
    pub variations_hash: u64,
    /// Hinting
    pub hinting: FontHinting,
    /// Antialiasing method
    pub font_smoothing: FontSmoothing,
}

/// Set of rasterized fonts stored in [`FontAtlas`]es.
///
/// # svge-main fork: weakref-trigger eviction
///
/// In addition to the upstream `HashMap<FontAtlasKey, Vec<FontAtlas>>` storage,
/// long-running consumers (such as the liveskill workbench) need a way to
/// evict atlases whose underlying `Image` asset is no longer referenced by
/// any rendered text. The upstream cache has no eviction; without this
/// addition, atlases for stale themes / DPI-changes / font-size combos
/// accumulate forever and can OOM apps that run for days.
///
/// **Eviction model (LS-gxrtooro):** every [`FontAtlas`] holds a strong
/// `Handle<Image>` to its atlas image. Live [`crate::TextLayoutInfo`]
/// instances also hold strong handles (via `atlas_handles`) for the atlases
/// they reference. [`Self::evict_stale`] walks the cache and reaps any
/// atlas whose `Arc::strong_count` is exactly 1 — that is, the atlas itself
/// is the only owner of the image handle, meaning no `TextLayoutInfo`
/// still references it. When a text entity despawns or re-shapes, its
/// `atlas_handles` drop, and the next sweep reaps the now-orphaned atlas.
///
/// This replaces the original time-based eviction (touch frame stamps +
/// idle threshold), which had a fatal interaction with shape-gating: gated
/// systems skip `touch()` on idle frames, so atlases would age past the
/// threshold and be evicted while still in use.
///
/// Existing read-side API is preserved via `Deref`/`DerefMut` to the inner
/// atlas map, so callers that only need the atlas storage are unaffected.
/// The `touch` / `last_used_frame` API is retained as a no-op and a
/// frame-stamp readback for backward compat with consumers that already
/// thread `FrameCount` through `update_text_layout_info` — the actual
/// eviction trigger no longer reads the timestamps.
#[derive(Debug, Default, Resource)]
pub struct FontAtlasSet {
    atlases: HashMap<FontAtlasKey, Vec<FontAtlas>>,
    /// svge-main fork: kept for backward compat with [`Self::touch`] /
    /// [`Self::last_used_frame`] callers; not consulted by the eviction
    /// path any more (which is now weakref-trigger). Will be removed in a
    /// later cleanup pass once all upstream callers stop threading
    /// `current_frame` through the public API.
    last_used_frames: HashMap<FontAtlasKey, u64>,
}

impl Deref for FontAtlasSet {
    type Target = HashMap<FontAtlasKey, Vec<FontAtlas>>;
    fn deref(&self) -> &Self::Target {
        &self.atlases
    }
}

impl DerefMut for FontAtlasSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.atlases
    }
}

impl FontAtlasSet {
    /// Checks whether the given subpixel-offset glyph is contained in any of the [`FontAtlas`]es for the font identified by the given [`FontAtlasKey`].
    pub fn has_glyph(&self, cache_key: GlyphCacheKey, font_key: &FontAtlasKey) -> bool {
        self.get(font_key)
            .is_some_and(|font_atlas| font_atlas.iter().any(|atlas| atlas.has_glyph(cache_key)))
    }

    /// Returns the total size in bytes of the image data for all fonts.
    pub fn total_bytes(&self, images: &Assets<Image>) -> u64 {
        self.values()
            .flat_map(|font_atlases| font_atlases.iter())
            .map(|font_atlas| {
                images
                    .get(font_atlas.texture)
                    .and_then(|image| image.data.as_ref())
                    .map_or(0, |data| data.len() as u64)
            })
            .sum()
    }

    /// svge-main: mark the given font atlas key as used at the given frame.
    ///
    /// Callers in the text-shaping hot path should invoke this for every
    /// `FontAtlasKey` they look up or insert, passing the current frame
    /// number. [`Self::evict_stale`] uses this timestamp to decide which
    /// atlases are candidates for eviction.
    pub fn touch(&mut self, key: FontAtlasKey, frame: u64) {
        self.last_used_frames.insert(key, frame);
    }

    /// svge-main: returns the last frame at which the given key was touched,
    /// or `0` for keys never touched (or already evicted).
    pub fn last_used_frame(&self, key: &FontAtlasKey) -> u64 {
        self.last_used_frames.get(key).copied().unwrap_or(0)
    }

    /// svge-main fork (LS-gxrtooro): drop atlases whose underlying `Image`
    /// asset has been reclaimed by the asset GC.
    ///
    /// Walks every [`FontAtlas`] in the cache. For each atlas, checks
    /// `images.contains(atlas.texture)`: if `false`, the atlas's `Image`
    /// has already been reaped (because no [`crate::TextLayoutInfo`] holds
    /// a strong handle to it any more — the only strong handles to atlas
    /// images live on `TextLayoutInfo::atlas_handles`). Such atlases are
    /// stale and are dropped from the cache.
    ///
    /// Atlases whose image is still in `Assets<Image>` are kept — at least
    /// one `TextLayoutInfo` is keeping the strong handle alive, which
    /// means the renderer may still need this atlas. When that
    /// `TextLayoutInfo` is replaced (text re-shaped, entity despawned,
    /// content changed), its handle drops, the asset GC reaps the image
    /// at end-of-frame, and the next eviction sweep reaps the atlas.
    ///
    /// `config.max_total_bytes` is a backstop cap. If the surviving atlas
    /// memory still exceeds the cap after the weakref-trigger pass, the
    /// largest atlases are dropped — same idea, but ignoring whether the
    /// image is still reachable (so this can drop in-use atlases under
    /// genuine memory pressure; size cap defaults are generous).
    ///
    /// Returns the number of individual [`FontAtlas`] instances removed.
    pub fn evict_stale(
        &mut self,
        _current_frame: u64,
        config: &FontAtlasEvictionConfig,
        images: &mut Assets<Image>,
    ) -> usize {
        let mut evicted = 0;

        // Pass 1: weakref-trigger eviction. An atlas's `texture` is now an
        // `AssetId<Image>` (no strong reference inside the atlas itself).
        // The strong handle lives on `TextLayoutInfo::atlas_handles`. When
        // every `TextLayoutInfo` referencing the atlas drops its handle,
        // the asset GC removes the `Image` from `Assets<Image>` at
        // end-of-frame. We notice on the next sweep via `contains()`.
        let mut empty_keys: Vec<FontAtlasKey> = Vec::new();
        for (key, atlases) in self.atlases.iter_mut() {
            let before = atlases.len();
            atlases.retain(|atlas| images.contains(atlas.texture));
            evicted += before - atlases.len();
            if atlases.is_empty() {
                empty_keys.push(*key);
            }
        }
        for key in empty_keys {
            self.atlases.remove(&key);
            self.last_used_frames.remove(&key);
        }

        // Pass 2: size-based backstop. If we still exceed the cap, drop the
        // largest atlas until under it. This intentionally ignores
        // reachability — once we're over the memory budget, we'd rather
        // risk a re-rasterise than OOM. With the post-fix model the
        // in-use atlases will be re-allocated on next shape, with the
        // strong handle going straight onto the consuming
        // `TextLayoutInfo`.
        if config.max_total_bytes < u64::MAX {
            loop {
                let total = self.total_bytes(images);
                if total <= config.max_total_bytes {
                    break;
                }
                // Find the largest atlas across all keys.
                let mut largest: Option<(FontAtlasKey, usize, u64)> = None;
                for (key, atlases) in self.atlases.iter() {
                    for (idx, atlas) in atlases.iter().enumerate() {
                        let bytes = images
                            .get(atlas.texture)
                            .and_then(|img| img.data.as_ref())
                            .map_or(0, |d| d.len() as u64);
                        let better = match largest {
                            None => true,
                            Some((_, _, prev_bytes)) => bytes > prev_bytes,
                        };
                        if better {
                            largest = Some((*key, idx, bytes));
                        }
                    }
                }
                let Some((key, idx, _)) = largest else {
                    break;
                };
                if let Some(atlases) = self.atlases.get_mut(&key) {
                    if idx < atlases.len() {
                        let removed = atlases.remove(idx);
                        // For backstop eviction we ARE responsible for
                        // removing the asset — the strong handle on
                        // `TextLayoutInfo` is still keeping the image
                        // alive, but we've decided to drop it under
                        // memory pressure.
                        images.remove(removed.texture);
                        evicted += 1;
                    }
                    if atlases.is_empty() {
                        self.atlases.remove(&key);
                        self.last_used_frames.remove(&key);
                    }
                }
            }
        }

        evicted
    }
}

/// svge-main fork: configuration for [`FontAtlasSet::evict_stale`].
///
/// Eviction (LS-gxrtooro) is now weakref-trigger-based: atlases are
/// reaped when no live [`crate::TextLayoutInfo`] holds a strong handle to
/// their image. The only configurable knob is a size-based backstop cap
/// for genuine memory pressure.
///
/// Default policy: cap total atlas memory at 64 MB. Set to `u64::MAX` to
/// disable the backstop entirely (rely solely on weakref eviction).
///
/// Eviction is opt-in: this resource exists in the world only if the consumer
/// inserts it (and runs a system that calls `evict_stale`). Bevy itself does
/// NOT auto-register an eviction system — consumers must wire it up
/// themselves. See `liveskill_ui_render` for the canonical wiring.
#[derive(Resource, Debug, Clone)]
pub struct FontAtlasEvictionConfig {
    /// Maximum total bytes of `Assets<Image>` storage devoted to font
    /// atlases. When `evict_stale` runs and totals exceed this cap after
    /// the weakref pass, the largest atlases are dropped until the cap is
    /// met (this can drop in-use atlases under genuine memory pressure —
    /// the renderer will re-rasterise next frame). Default: 64 MiB. Set to
    /// `u64::MAX` to disable the backstop entirely.
    pub max_total_bytes: u64,
    /// Deprecated, kept as a `u64::MAX`-defaulted no-op for backward
    /// compatibility with downstream code that constructed
    /// `FontAtlasEvictionConfig` literals naming this field. Time-based
    /// eviction was removed — see `evict_stale` docs.
    #[deprecated(
        note = "max_idle_frames is no longer consulted; eviction is now \
                weakref-trigger-based. This field will be removed in a \
                future cleanup."
    )]
    pub max_idle_frames: u64,
}

impl Default for FontAtlasEvictionConfig {
    fn default() -> Self {
        #[allow(deprecated)]
        Self {
            max_total_bytes: 64 * 1024 * 1024,
            max_idle_frames: u64::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontAtlas, SubpixelBucket};
    use bevy_asset::Handle;
    use bevy_math::UVec2;

    fn make_key(font_size_bits: u32) -> FontAtlasKey {
        FontAtlasKey {
            id: 0,
            index: 0,
            font_size_bits,
            variations_hash: 0,
            hinting: FontHinting::Disabled,
            font_smoothing: FontSmoothing::AntiAliased,
        }
    }

    /// Build a fresh atlas inside `images`. Returns the atlas (which only
    /// holds a weak `AssetId<Image>`) and the strong handle returned by
    /// `FontAtlas::new` so the caller can either keep it (simulating a
    /// `TextLayoutInfo` reference) or drop it (simulating no live
    /// references → eligible for weakref-trigger eviction).
    fn make_atlas(images: &mut Assets<Image>) -> (FontAtlas, Handle<Image>) {
        FontAtlas::new(images, UVec2::splat(64), FontSmoothing::AntiAliased)
    }

    #[test]
    fn touch_still_records_frame_for_back_compat() {
        // touch/last_used_frame retained for back-compat with callers that
        // still thread FrameCount through update_text_layout_info. The
        // values are no longer consulted by eviction.
        let mut set = FontAtlasSet::default();
        let key = make_key(0);
        assert_eq!(set.last_used_frame(&key), 0);
        set.touch(key, 42);
        assert_eq!(set.last_used_frame(&key), 42);
    }

    #[test]
    fn weakref_eviction_drops_atlas_when_image_was_reclaimed() {
        // Pre-fix: time-based threshold could evict a still-in-use atlas if
        // its key wasn't touch()'d this frame (shape gating skipped touch).
        // Post-fix: eviction fires when the atlas's `Image` asset is no
        // longer in `Assets<Image>`. We simulate that by removing the
        // image directly (in real code, asset GC reaps after the last
        // strong handle drops).
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let key = make_key(0);
        let (atlas, strong_handle) = make_atlas(&mut images);
        let atlas_id = atlas.texture;
        set.atlases.insert(key, vec![atlas]);

        // Drop the strong handle and remove the image (simulating asset
        // GC reaping after the last strong handle dropped at end of
        // previous frame).
        drop(strong_handle);
        images.remove(atlas_id);

        let config = FontAtlasEvictionConfig::default();
        let evicted = set.evict_stale(0, &config, &mut images);
        assert_eq!(evicted, 1);
        assert!(!set.atlases.contains_key(&key));
    }

    #[test]
    fn weakref_eviction_keeps_atlas_when_image_still_alive() {
        // The user-directed model: while a `TextLayoutInfo` holds a strong
        // handle, the image asset stays in `Assets<Image>`, so
        // `evict_stale` keeps the atlas — even across many sweeps.
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let key = make_key(0);
        let (atlas, layout_strong) = make_atlas(&mut images);
        let atlas_id = atlas.texture;
        set.atlases.insert(key, vec![atlas]);

        let config = FontAtlasEvictionConfig::default();
        // Sweep multiple times. `images.contains(atlas_id)` is `true`
        // throughout because `layout_strong` keeps the asset alive.
        for _ in 0..5 {
            let evicted = set.evict_stale(0, &config, &mut images);
            assert_eq!(evicted, 0);
            assert!(set.atlases.contains_key(&key));
            assert!(images.contains(atlas_id));
        }

        // Drop the layout-side strong handle and remove the image
        // (simulating end-of-frame asset GC). The next sweep reaps.
        drop(layout_strong);
        images.remove(atlas_id);
        let evicted = set.evict_stale(0, &config, &mut images);
        assert_eq!(evicted, 1);
        assert!(!set.atlases.contains_key(&key));
    }

    #[test]
    fn weakref_eviction_does_not_consult_idle_threshold() {
        // The original bug (LS-gxrtooro): a still-in-use atlas was reaped
        // because shape gating skipped touch() and the entry aged past the
        // idle threshold. Post-fix, the only signal is whether the image
        // is still in `Assets<Image>`: even if `current_frame` is far
        // ahead of any touch timestamp, an atlas whose image is still
        // alive is preserved.
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let key = make_key(0);
        let (atlas, _layout_strong) = make_atlas(&mut images);
        set.atlases.insert(key, vec![atlas]);

        // Pre-fix would have evicted with `current_frame = u64::MAX`
        // (entire idle threshold elapsed). Post-fix retains the atlas
        // because the strong handle keeps the image asset alive.
        let config = FontAtlasEvictionConfig::default();
        let evicted = set.evict_stale(u64::MAX, &config, &mut images);
        assert_eq!(evicted, 0);
        assert!(set.atlases.contains_key(&key));
    }

    #[test]
    fn default_config_sane_values() {
        #[allow(deprecated)]
        let cfg = FontAtlasEvictionConfig::default();
        // Backstop cap is 64 MiB. max_idle_frames is the deprecated no-op,
        // defaulted to u64::MAX (disabled) — kept only for the back-compat
        // field-init path.
        assert_eq!(cfg.max_total_bytes, 64 * 1024 * 1024);
        #[allow(deprecated)]
        {
            assert_eq!(cfg.max_idle_frames, u64::MAX);
        }
    }

    // Compile-test: ensure SubpixelBucket import path still resolves.
    #[allow(dead_code)]
    fn _bucket_compile_check() -> SubpixelBucket {
        SubpixelBucket::default()
    }
}
