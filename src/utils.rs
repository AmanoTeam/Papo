use std::{
    collections::HashMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

use adw::prelude::*;
use fast_qr::{
    ECL, QRBuilder,
    convert::{Builder, Shape, image::ImageBuilder},
};
use glib::Bytes;
use glycin::{Loader, MemoryFormatSelection};
use gtk::{gdk, gio::File, glib};
use jiff::{Zoned, civil::Date};
use relm4::prelude::*;
use rlibphonenumber::{PhoneNumber, PhoneNumberFormat};

use crate::i18n;

pub fn get_first_name(name: &str) -> String {
    if name.is_empty() {
        String::new()
    } else if name.contains(' ') {
        let (first, _) = name.split_once(' ').unwrap();

        first.to_string()
    } else {
        name.to_string()
    }
}

pub async fn generate_qr_code(data: &str, size: u32) -> Result<gdk::Texture, Box<dyn Error>> {
    let data = data.to_string();
    let bytes = relm4::spawn_blocking(move || {
        let qr = QRBuilder::new(data.as_str())
            .ecl(ECL::H)
            .build()
            .expect("Failed to generate QR code");
        ImageBuilder::default()
            .shape(Shape::Circle)
            .fit_width(size)
            .fit_height(size)
            .to_bytes(&qr)
            .expect("Failed to build QR code image")
    })
    .await
    .expect("QR generation task panicked");

    // Load the image through glycin.
    let loader = Loader::new_bytes(Bytes::from_owned(bytes));
    let mut image_doc = loader.load().await?;
    let frame = image_doc.next_frame().await?;
    let texture = frame.texture();

    Ok(texture)
}

/// Formats a date into a human-readable label for date separators.
pub fn format_date_label(date: Date) -> String {
    let today = Zoned::now().date();

    if date == today {
        return i18n!("Today");
    }

    if let Some(yesterday) = today.yesterday().ok()
        && date == yesterday
    {
        return i18n!("Yesterday");
    }

    // Same year: "February 23", different year: "February 23, 2024".
    if date.year() == today.year() {
        date.strftime("%B %-e").to_string()
    } else {
        date.strftime("%B %-e, %Y").to_string()
    }
}

/// Formats a LID as international phone number.
pub fn format_lid_as_number(lid: &str) -> String {
    let phone = extract_phone_from_jid(lid);

    phone.parse::<PhoneNumber>().map_or(phone, |number| {
        number
            .format_as(PhoneNumberFormat::International)
            .to_string()
    })
}

/// Strips the device suffix from a JID (`user:device@server` -> `user@server`).
pub fn bare_jid(jid: &str) -> String {
    match jid.split_once('@') {
        Some((user, server)) => {
            let user = user.split(':').next().unwrap_or(user);
            format!("{user}@{server}")
        }
        None => jid.to_string(),
    }
}

/// Whether the jid addresses a contact by its linked-device identity.
pub fn is_lid_jid(jid: &str) -> bool {
    jid.ends_with("@lid")
}

/// Whether the jid addresses a contact by their phone number.
pub fn is_phone_jid(jid: &str) -> bool {
    jid.ends_with("@s.whatsapp.net") || jid.ends_with("@c.us")
}

/// Extracts phone number from JID/LID.
pub fn extract_phone_from_jid(jid: &str) -> String {
    format!("+{}", jid.split('@').next().unwrap_or(jid))
}

/// Edge length in pixels that avatar textures are downscaled to fit.
/// Full-resolution profile pictures would otherwise live in texture
/// memory at megabytes each, and the chat list reloads them on every
/// update.
const AVATAR_MAX_EDGE: usize = 96;

type AvatarCache = HashMap<PathBuf, (SystemTime, gdk::Texture)>;

fn avatar_cache() -> &'static Mutex<AvatarCache> {
    static CACHE: OnceLock<Mutex<AvatarCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns the cached avatar texture for a path if it has been decoded
/// before and the file has not changed since.
pub fn cached_avatar<P: AsRef<Path>>(path: P) -> Option<gdk::Texture> {
    let path = path.as_ref().to_path_buf();
    let mtime = fs::metadata(&path).ok()?.modified().ok()?;
    let cache = avatar_cache().lock().expect("avatar cache poisoned");

    cache
        .get(&path)
        .filter(|(cached_mtime, _)| *cached_mtime == mtime)
        .map(|(_, texture)| texture.clone())
}

/// Load an avatar into a texture from a local path, downscaled to keep
/// texture memory small. Textures are cached by path until the file
/// changes, so repeated chat list updates only bump the reference count.
pub async fn load_avatar<P: AsRef<Path>>(path: P) -> Option<gdk::Texture> {
    let path = path.as_ref().to_path_buf();
    let mtime = fs::metadata(&path).ok()?.modified().ok()?;

    {
        let cache = avatar_cache().lock().expect("avatar cache poisoned");
        if let Some((cached_mtime, texture)) = cache.get(&path)
            && *cached_mtime == mtime
        {
            return Some(texture.clone());
        }
    }

    let file = File::for_path(&path);
    let (bytes, _) = match file.load_bytes_future().await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Failed to load avatar from {}: {e}", path.display());

            return None;
        }
    };

    let texture = downscale_avatar(bytes).await;
    if let Some(texture) = texture.clone() {
        avatar_cache()
            .lock()
            .expect("avatar cache poisoned")
            .insert(path, (mtime, texture));
    }

    texture
}

/// Decodes an image and box-filter downscales it so the longest edge
/// fits within [`AVATAR_MAX_EDGE`], producing a small texture instead of
/// a full-resolution one.
async fn downscale_avatar(bytes: Bytes) -> Option<gdk::Texture> {
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
        gdk::MemoryFormat::R8g8b8a8Premultiplied,
        &Bytes::from_owned(out),
        new_width * 4,
    );

    Some(texture.upcast())
}
