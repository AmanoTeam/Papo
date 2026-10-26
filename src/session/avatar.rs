//! The avatar system, in one place: the on-disk file cache, the
//! decoded-texture memory cache, and the decode pipeline that turns
//! files into textures.
//!
//! Files are keyed by the jid they were requested for (bare form for
//! senders, full form for chats — the same key family either way).
//! Textures are cached in memory by path until the file's mtime
//! changes. Decodes run in a glycin sandbox process, bounded by a
//! global concurrency budget, an in-flight gate per file, and a
//! backoff after failures — rebinding storms must not multiply
//! sandbox processes. Callers that find their file busy coalesce
//! onto the in-flight decode instead of wasting the bind.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use adw::prelude::*;
use glib::Bytes;
use glycin::{Loader, MemoryFormatSelection};
use gtk::{
    gdk::{self, MemoryFormat},
    gio::File,
    glib,
};
use relm4::gtk;
use tokio::sync::{Semaphore, watch};

use crate::{CACHE_DIR, utils::bare_jid};

/// Edge length in pixels that avatar textures are downscaled to fit.
const AVATAR_MAX_EDGE: usize = 96;

/// Bounds concurrent avatar decodes: each one runs in a glycin sandbox
/// process that holds pipes and an epoll context, so an uncapped burst
/// of them exhausts the process fd table.
const AVATAR_MAX_CONCURRENT_DECODES: usize = 4;

/// How long a failed decode suppresses retries for its file; without
/// it, a corrupt file would pay a sandbox spawn on every bind.
const AVATAR_FAILURE_BACKOFF: Duration = Duration::from_secs(60);

/// Directory backing store for avatars downloaded from `WhatsApp`.
#[derive(Clone, Debug)]
pub struct AvatarCache {
    /// Directory where avatars are stored.
    cache_dir: PathBuf,
}

impl AvatarCache {
    const TTL_TIME: Duration = Duration::new(7 * 24 * 60 * 60, 0);

    pub fn new() -> Result<Self, io::Error> {
        let cache_dir = CACHE_DIR.join("avatars");
        fs::create_dir_all(&cache_dir)?;

        Ok(Self { cache_dir })
    }

    pub fn is_cached(&self, jid: &str) -> bool {
        self.avatar_path(jid).exists()
    }

    pub fn is_stale(&self, jid: &str) -> bool {
        fs::metadata(self.avatar_path(jid)).map_or(true, |m| {
            m.modified()
                .map_or(true, |t| t.elapsed().is_ok_and(|e| e > Self::TTL_TIME))
        })
    }

    pub fn avatar_path(&self, jid: &str) -> PathBuf {
        // Sanitize JID for use as filename.
        let safe_jid = jid.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.cache_dir.join(format!("{safe_jid}.jpg"))
    }

    /// Get the cached avatar path if it exists.
    pub fn cached_path(&self, jid: &str) -> Option<String> {
        let path = self.avatar_path(jid);
        path.exists().then(|| path.to_string_lossy().into_owned())
    }

    pub fn save(&self, jid: &str, data: &[u8]) -> Result<String, io::Error> {
        let path = self.avatar_path(jid);
        fs::write(&path, data)?;

        Ok(path.to_string_lossy().into_owned())
    }

    pub fn delete(&self, jid: &str) -> Result<(), io::Error> {
        let path = self.avatar_path(jid);
        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    pub fn clear(&self) -> Result<(), io::Error> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
            fs::create_dir_all(&self.cache_dir)?;
        }

        Ok(())
    }
}

/// The process-wide file cache. Constructed once; a missing cache
/// directory disables the whole system gracefully.
pub fn global() -> Option<&'static AvatarCache> {
    static CACHE: OnceLock<Option<AvatarCache>> = OnceLock::new();
    CACHE.get_or_init(|| AvatarCache::new().ok()).as_ref()
}

/// Returns the on-disk avatar file for a sender jid when a picture has
/// been fetched for it before — either as a chat avatar or through the
/// sender-avatar request flow, which share the same file key.
pub fn sender_cached_path(jid: &str) -> Option<String> {
    global()?.cached_path(&bare_jid(jid))
}

type TextureCache = HashMap<PathBuf, (SystemTime, gdk::Texture)>;

fn texture_cache() -> &'static Mutex<TextureCache> {
    static CACHE: OnceLock<Mutex<TextureCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Default)]
struct LoadGate {
    in_flight: HashMap<PathBuf, watch::Receiver<()>>,
    failed_at: HashMap<PathBuf, Instant>,
}

fn load_gate() -> &'static Mutex<LoadGate> {
    static GATE: OnceLock<Mutex<LoadGate>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(LoadGate::default()))
}

fn decode_permits() -> &'static Semaphore {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| Semaphore::new(AVATAR_MAX_CONCURRENT_DECODES))
}

/// Returns the avatar texture for a path if it has been decoded before
/// and the file has not changed since.
pub fn texture<P: AsRef<Path>>(path: P) -> Option<gdk::Texture> {
    let path = path.as_ref().to_path_buf();
    let mtime = fs::metadata(&path).ok()?.modified().ok()?;
    let cache = texture_cache()
        .lock()
        .expect("avatar texture cache poisoned");

    cache
        .get(&path)
        .filter(|(cached_mtime, _)| *cached_mtime == mtime)
        .map(|(_, texture)| texture.clone())
}

/// Load an avatar into a texture from a local path, downscaled to keep
/// texture memory small. Textures are cached by path until the file
/// changes, so repeated chat list updates only bump the reference
/// count.
///
/// Loads are bounded: one decode per file, a small global concurrency
/// budget, and a backoff after failures — a flurry of rebinds must not
/// multiply sandbox decodes. Callers that find their file busy coalesce
/// onto the in-flight decode instead of wasting the bind.
pub async fn load<P: AsRef<Path>>(path: P) -> Option<gdk::Texture> {
    let path = path.as_ref().to_path_buf();

    loop {
        let mtime = fs::metadata(&path).ok()?.modified().ok()?;

        {
            let cache = texture_cache()
                .lock()
                .expect("avatar texture cache poisoned");
            if let Some((cached_mtime, texture)) = cache.get(&path)
                && *cached_mtime == mtime
            {
                return Some(texture.clone());
            }
        }

        // `Ok` carries the leader's sender, kept alive until the decode
        // result is booked; `Err` carries a subscription to wait on.
        let leader = {
            let mut gate = load_gate().lock().expect("avatar load gate poisoned");
            let backed_off = gate
                .failed_at
                .get(&path)
                .is_some_and(|at| at.elapsed() < AVATAR_FAILURE_BACKOFF);
            if backed_off {
                tracing::trace!("Avatar load backing off: {}", path.display());
                return None;
            }

            match gate.in_flight.get(&path) {
                // The leader died mid-decode (a cancelled future): take
                // over instead of waiting on a closed channel.
                Some(rx) if rx.has_changed().is_err() => {
                    let (tx, rx) = watch::channel(());
                    gate.in_flight.insert(path.clone(), rx);

                    Ok(tx)
                }
                Some(rx) => Err(rx.clone()),
                None => {
                    let (tx, rx) = watch::channel(());
                    gate.in_flight.insert(path.clone(), rx);

                    Ok(tx)
                }
            }
        };

        let _leader = match leader {
            Ok(tx) => tx,
            Err(mut rx) => {
                // Coalesce on the in-flight decode: once it lands, the
                // next pass finds the texture, the failure backoff, or
                // a dead leader to replace.
                let _ = rx.changed().await;
                continue;
            }
        };

        let texture = decode(&path).await;

        if let Some(texture) = texture.clone() {
            texture_cache()
                .lock()
                .expect("avatar texture cache poisoned")
                .insert(path.clone(), (mtime, texture));
        }

        {
            let mut gate = load_gate().lock().expect("avatar load gate poisoned");
            gate.in_flight.remove(&path);

            match &texture {
                Some(_) => {
                    gate.failed_at.remove(&path);
                }
                None => {
                    gate.failed_at.insert(path.clone(), Instant::now());
                }
            }
        }

        return texture;
    }
}

/// Reads the file and decodes it inside the sandbox, bounded by the
/// global decode permits.
async fn decode(path: &Path) -> Option<gdk::Texture> {
    let _permit = decode_permits()
        .acquire()
        .await
        .expect("avatar decode permits are never closed");

    let file = File::for_path(path);
    let (bytes, _) = match file.load_bytes_future().await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Failed to load avatar from {}: {e}", path.display());
            return None;
        }
    };

    downscale(bytes).await
}

/// Decodes an image and box-filter downscales it so the longest edge
/// fits within [`AVATAR_MAX_EDGE`], producing a small texture instead
/// of a full-resolution one.
async fn downscale(bytes: Bytes) -> Option<gdk::Texture> {
    let mut loader = Loader::new_bytes(bytes);
    loader.accepted_memory_formats(MemoryFormatSelection::R8g8b8a8Premultiplied);
    let mut image = loader.load().await.ok()?;
    let frame = image.next_frame().await.ok()?;

    let width = usize::try_from(frame.width()).ok()?;
    let height = usize::try_from(frame.height()).ok()?;
    let stride = usize::try_from(frame.stride()).ok()?;
    if width == 0 || height == 0 || stride < width * 4 {
        return None;
    }

    if width.max(height) <= AVATAR_MAX_EDGE {
        return Some(frame.texture());
    }

    let long_edge = width.max(height);
    let new_width = (width * AVATAR_MAX_EDGE / long_edge).max(1);
    let new_height = (height * AVATAR_MAX_EDGE / long_edge).max(1);

    let src = frame.buf_bytes();
    let src = src.as_ref();
    if src.len() < height * stride {
        return None;
    }

    // Box filter: every destination pixel averages the source box that
    // maps onto it. The frame is 8-bit RGBA premultiplied, so averaging
    // bytes per channel position is valid.
    let mut out = vec![0u8; new_width * new_height * 4];
    for ty in 0..new_height {
        let y0 = ty * height / new_height;
        let y1 = ((ty + 1) * height / new_height).clamp(y0 + 1, height);

        for tx in 0..new_width {
            let x0 = tx * width / new_width;
            let x1 = ((tx + 1) * width / new_width).clamp(x0 + 1, width);

            let (mut r, mut g, mut b, mut a, mut count) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                let row = y * stride;

                for x in x0..x1 {
                    let px = row + x * 4;
                    r += u64::from(src[px]);
                    g += u64::from(src[px + 1]);
                    b += u64::from(src[px + 2]);
                    a += u64::from(src[px + 3]);
                    count += 1;
                }
            }

            let px = (ty * new_width + tx) * 4;
            out[px] = u8::try_from(r / count).ok()?;
            out[px + 1] = u8::try_from(g / count).ok()?;
            out[px + 2] = u8::try_from(b / count).ok()?;
            out[px + 3] = u8::try_from(a / count).ok()?;
        }
    }

    let texture = gdk::MemoryTexture::new(
        i32::try_from(new_width).ok()?,
        i32::try_from(new_height).ok()?,
        MemoryFormat::R8g8b8a8Premultiplied,
        &Bytes::from_owned(out),
        new_width * 4,
    );

    Some(texture.upcast())
}
