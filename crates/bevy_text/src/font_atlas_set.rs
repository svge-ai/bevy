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
/// # svge-main fork: per-key LRU tracking
///
/// In addition to the upstream `HashMap<FontAtlasKey, Vec<FontAtlas>>` storage,
/// this fork tracks `last_used_frames: HashMap<FontAtlasKey, u64>` so that
/// long-running consumers (such as the liveskill workbench) can periodically
/// evict atlases for font configurations that haven't been touched in a long
/// time. The upstream cache has no eviction; without this addition, atlases
/// for stale themes / DPI-changes / font-size combos accumulate forever and
/// can OOM apps that run for days or weeks.
///
/// Existing read-side API is preserved via `Deref`/`DerefMut` to the inner
/// atlas map, so callers that only need the atlas storage are unaffected.
/// New callers that want LRU should call [`Self::touch`] from the lookup
/// hot path and [`Self::evict_stale`] from a periodic system.
#[derive(Debug, Default, Resource)]
pub struct FontAtlasSet {
    atlases: HashMap<FontAtlasKey, Vec<FontAtlas>>,
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
                    .get(&font_atlas.texture)
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

    /// svge-main: drop atlases per the given eviction config.
    ///
    /// Eviction is a two-pass walk:
    ///
    /// 1. **Time-based:** any key whose `last_used_frame` is more than
    ///    `config.max_idle_frames` older than `current_frame` is dropped
    ///    (along with all its atlases and the underlying `Image` handles).
    ///    Set `max_idle_frames = u64::MAX` to disable.
    /// 2. **Size-based LRU:** if the surviving total atlas memory still
    ///    exceeds `config.max_total_bytes`, the oldest-touched key is
    ///    repeatedly evicted until the cap is met. Set `max_total_bytes =
    ///    u64::MAX` to disable.
    ///
    /// Returns the number of `FontAtlasKey` entries removed (not the number
    /// of individual `FontAtlas` instances — each key may have several).
    ///
    /// `images` is required because the atlas owns a `Handle<Image>`; the
    /// underlying `Image` asset must also be removed from the assets
    /// collection or the texture memory itself wouldn't be reclaimed.
    pub fn evict_stale(
        &mut self,
        current_frame: u64,
        config: &FontAtlasEvictionConfig,
        images: &mut Assets<Image>,
    ) -> usize {
        let mut evicted = 0;

        // Pass 1: time-based eviction.
        if config.max_idle_frames < u64::MAX {
            let cutoff = current_frame.saturating_sub(config.max_idle_frames);
            // Collect-then-remove avoids invalidating the iterator over
            // last_used_frames while we mutate atlases + images.
            let stale_keys: Vec<FontAtlasKey> = self
                .last_used_frames
                .iter()
                .filter_map(|(k, &frame)| if frame < cutoff { Some(*k) } else { None })
                .collect();
            for key in stale_keys {
                if self.evict_key(&key, images) {
                    evicted += 1;
                }
            }
        }

        // Pass 2: size-based LRU eviction (drops oldest-touched until under cap).
        if config.max_total_bytes < u64::MAX {
            loop {
                let total = self.total_bytes(images);
                if total <= config.max_total_bytes {
                    break;
                }
                let oldest_key = self
                    .last_used_frames
                    .iter()
                    .min_by_key(|&(_, &frame)| frame)
                    .map(|(k, _)| *k);
                let Some(key) = oldest_key else {
                    // No tracked keys remain but bytes still over cap — nothing
                    // we can do without breaking untracked entries. Bail.
                    break;
                };
                if !self.evict_key(&key, images) {
                    // Defensive: if the key vanished mid-iteration, stop to
                    // avoid an infinite loop.
                    break;
                }
                evicted += 1;
            }
        }

        evicted
    }

    /// Internal helper: drop a single key's atlases and texture handles.
    /// Returns `true` if the key was present.
    fn evict_key(&mut self, key: &FontAtlasKey, images: &mut Assets<Image>) -> bool {
        let removed = self.atlases.remove(key);
        self.last_used_frames.remove(key);
        if let Some(atlases) = removed {
            for atlas in atlases {
                images.remove(&atlas.texture);
            }
            true
        } else {
            false
        }
    }
}

/// svge-main: configuration for [`FontAtlasSet::evict_stale`].
///
/// Default policy is hybrid: evict atlases idle for ≥ 5 minutes at 60 fps
/// (`60 * 60 * 5` frames) AND cap total atlas memory at 64 MB. Either knob
/// can be disabled by setting it to `u64::MAX`.
///
/// Eviction is opt-in: this resource exists in the world only if the consumer
/// inserts it (and runs a system that calls `evict_stale`). Bevy itself does
/// NOT auto-register an eviction system — consumers must wire it up
/// themselves. See `liveskill_ui_render` for the canonical wiring.
#[derive(Resource, Debug, Clone)]
pub struct FontAtlasEvictionConfig {
    /// Atlases not touched in this many frames are evicted. Default:
    /// `60 * 60 * 5` (~5 minutes at 60 fps). Set to `u64::MAX` to disable
    /// time-based eviction.
    pub max_idle_frames: u64,
    /// Maximum total bytes of `Assets<Image>` storage devoted to font
    /// atlases. When `evict_stale` runs and totals exceed this cap, atlases
    /// are dropped LRU-first until the cap is met. Default: 64 MiB. Set to
    /// `u64::MAX` to disable size-based eviction.
    pub max_total_bytes: u64,
}

impl Default for FontAtlasEvictionConfig {
    fn default() -> Self {
        Self {
            max_idle_frames: 60 * 60 * 5,
            max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SubpixelBucket;

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

    #[test]
    fn touch_records_frame() {
        let mut set = FontAtlasSet::default();
        let key = make_key(0);
        assert_eq!(set.last_used_frame(&key), 0);
        set.touch(key, 42);
        assert_eq!(set.last_used_frame(&key), 42);
        set.touch(key, 100);
        assert_eq!(set.last_used_frame(&key), 100);
    }

    #[test]
    fn evict_stale_drops_idle_keys() {
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let recent = make_key(0);
        let stale = make_key(1);
        // Insert empty atlas vecs so eviction has something to remove from
        // the atlas map (touch-only entries also get pruned from
        // last_used_frames, but we want both maps in lockstep).
        set.atlases.insert(recent, Vec::new());
        set.atlases.insert(stale, Vec::new());
        set.touch(recent, 1000);
        set.touch(stale, 100);

        // current_frame = 1100, max_idle = 200 -> cutoff = 900.
        // recent at 1000 > 900 (kept). stale at 100 < 900 (evicted).
        let config = FontAtlasEvictionConfig {
            max_idle_frames: 200,
            max_total_bytes: u64::MAX,
        };
        let evicted = set.evict_stale(1100, &config, &mut images);
        assert_eq!(evicted, 1);
        assert!(set.atlases.contains_key(&recent));
        assert!(!set.atlases.contains_key(&stale));
        assert_eq!(set.last_used_frame(&recent), 1000);
        assert_eq!(set.last_used_frame(&stale), 0);
    }

    #[test]
    fn evict_stale_disabled_when_max_idle_is_u64_max() {
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let key = make_key(0);
        set.atlases.insert(key, Vec::new());
        set.touch(key, 0);

        let config = FontAtlasEvictionConfig {
            max_idle_frames: u64::MAX,
            max_total_bytes: u64::MAX,
        };
        let evicted = set.evict_stale(u64::MAX, &config, &mut images);
        assert_eq!(evicted, 0);
        assert!(set.atlases.contains_key(&key));
    }

    #[test]
    fn evict_stale_handles_underflow_at_low_current_frame() {
        // current_frame = 50, max_idle = 200. saturating_sub -> 0. No keys
        // with frame < 0, so nothing is evicted (correct: at session start
        // we shouldn't evict anything).
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let key = make_key(0);
        set.atlases.insert(key, Vec::new());
        set.touch(key, 10);

        let config = FontAtlasEvictionConfig {
            max_idle_frames: 200,
            max_total_bytes: u64::MAX,
        };
        let evicted = set.evict_stale(50, &config, &mut images);
        assert_eq!(evicted, 0);
        assert!(set.atlases.contains_key(&key));
    }

    #[test]
    fn evict_stale_drops_last_used_frame_entries_for_keys_with_no_atlases() {
        // If touch() has been called but the atlas map never got an entry
        // (e.g. the lookup was for a font that failed to shape), the stale
        // entry in last_used_frames should still be cleaned up so the map
        // doesn't grow unbounded.
        let mut set = FontAtlasSet::default();
        let mut images = Assets::<Image>::default();
        let key = make_key(0);
        set.touch(key, 100);
        assert_eq!(set.last_used_frame(&key), 100);

        let config = FontAtlasEvictionConfig {
            max_idle_frames: 50,
            max_total_bytes: u64::MAX,
        };
        // current_frame=200, cutoff=150. key was touched at 100 < 150 -> evict.
        let evicted = set.evict_stale(200, &config, &mut images);
        // Returns 0 because evict_key only counts when the key was in `atlases`.
        assert_eq!(evicted, 0);
        // But last_used_frames was still cleaned up.
        assert_eq!(set.last_used_frame(&key), 0);
    }

    #[test]
    fn default_config_sane_values() {
        let cfg = FontAtlasEvictionConfig::default();
        assert!(cfg.max_idle_frames > 0);
        assert!(cfg.max_total_bytes > 0);
        // 5 minutes at 60 fps = 18000 frames.
        assert_eq!(cfg.max_idle_frames, 18000);
        // 64 MiB.
        assert_eq!(cfg.max_total_bytes, 64 * 1024 * 1024);
    }

    // Compile-test: ensure SubpixelBucket import path still resolves.
    #[allow(dead_code)]
    fn _bucket_compile_check() -> SubpixelBucket {
        SubpixelBucket::default()
    }
}
