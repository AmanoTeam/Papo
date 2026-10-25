use std::{
    collections::{HashMap, HashSet},
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

use crate::{
    db::entities::Contact, db::store::SessionStore, i18n, state::Chat, state::ChatMessage,
};

/// Edge length in pixels that avatar textures are downscaled to fit.
/// Full-resolution profile pictures would otherwise live in texture
/// memory at megabytes each, and the chat list reloads them on every
/// update.
const AVATAR_MAX_EDGE: usize = 96;

type AvatarCache = HashMap<PathBuf, (SystemTime, gdk::Texture)>;

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

/// Whether the jid addresses the shared status conversation, which
/// every client must keep out of its chat list: statuses arrive as
/// ordinary messages addressed to one feed jid.
pub fn is_status_broadcast(jid: &str) -> bool {
    bare_jid(jid) == "status@broadcast"
}

/// Extracts phone number from JID/LID.
pub fn extract_phone_from_jid(jid: &str) -> String {
    format!("+{}", jid.split('@').next().unwrap_or(jid))
}

/// Whether a wire-sourced "name" is really the sender's jid: an
/// `@` form, the bare jid itself, or any letterless digit run.
pub(crate) fn looks_like_jid(candidate: &str, sender_bare: &str) -> bool {
    candidate.contains('@')
        || candidate == sender_bare
        || (!candidate.chars().any(char::is_alphabetic)
            && candidate.chars().any(|c| c.is_ascii_digit()))
}

/// One sender's best stored name: a real contact name plain, or a
/// learned push name tilde-prefixed.
/// A contact's display name: the real name, or the learned push
/// name marked the `WhatsApp` way.
fn contact_display_name(contact: &Contact) -> Option<String> {
    if let Some(name) = contact.name.as_deref().filter(|name| !name.is_empty()) {
        return Some(name.to_string());
    }

    contact
        .push_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(|name| format!("~{name}"))
}

/// One sender's resolved name across both of their jid identities,
/// resolved against a snapshot of the contacts table.
fn resolve_stored_name(
    contacts: &HashMap<String, Contact>,
    lid_to_pn: &HashMap<String, String>,
    pn_to_lids: &HashMap<String, Vec<String>>,
    bare: &str,
) -> Option<String> {
    if let Some(contact) = contacts.get(bare)
        && let Some(name) = contact_display_name(contact)
    {
        return Some(name);
    } else if is_lid_jid(bare)
        && let Some(phone) = lid_to_pn.get(bare)
        && let Some(contact) = contacts.get(phone)
        && let Some(name) = contact_display_name(contact)
    {
        return Some(name);
    } else if is_phone_jid(bare)
        && let Some(lids) = pn_to_lids.get(bare)
    {
        for lid in lids {
            if let Some(contact) = contacts.get(lid)
                && let Some(name) = contact_display_name(contact)
            {
                return Some(name);
            }
        }
    }

    None
}

/// Reusable sender-resolution state: one snapshot of the contacts
/// and LID tables that every message of a load — or every preview
/// of a list populate — resolves against, so a whole pass costs two
/// queries instead of two per sender.
pub(crate) struct SenderResolution {
    contacts: HashMap<String, Contact>,
    lid_to_pn: HashMap<String, String>,
    pn_to_lids: HashMap<String, Vec<String>>,
    /// Digit-keyed index of the same pairs, handed to the timeline
    /// so one person's LID-keyed and phone-numbered rows group as
    /// one author.
    lid_digits_to_phone: HashMap<String, String>,
}

impl SenderResolution {
    pub(crate) async fn new(db: &SessionStore) -> Self {
        let contacts = db
            .get_all_contacts()
            .await
            .map(|all| {
                all.into_iter()
                    .map(|contact| (contact.jid.clone(), contact))
                    .collect::<HashMap<String, Contact>>()
            })
            .unwrap_or_default();
        let mappings = db.get_all_lid_mappings().await;
        let lid_to_pn = mappings
            .iter()
            .map(|(lid, phone)| (format!("{lid}@lid"), format!("{phone}@s.whatsapp.net")))
            .collect::<HashMap<String, String>>();

        let mut pn_to_lids = HashMap::<String, Vec<String>>::new();
        for (lid, phone) in &mappings {
            pn_to_lids
                .entry(format!("{phone}@s.whatsapp.net"))
                .or_default()
                .push(format!("{lid}@lid"));
        }

        let lid_digits_to_phone = mappings
            .iter()
            .map(|(lid, phone)| (lid.clone(), phone.clone()))
            .collect::<HashMap<String, String>>();

        Self {
            contacts,
            lid_to_pn,
            pn_to_lids,
            lid_digits_to_phone,
        }
    }

    /// The digit-keyed LID identity map: `lid digits → phone digits`
    /// for every known mapping pair, including any learned during
    /// this snapshot's lifetime.
    pub(crate) fn identities(&self) -> HashMap<String, String> {
        self.lid_digits_to_phone.clone()
    }

    /// A 1:1 chat keyed by one identity that carries its peer under
    /// the other identity is itself proof of the mapping pair; learn
    /// it here rather than waiting for a sync chunk to carry it.
    /// Pairs the snapshot already knows are skipped, so bulk
    /// preview resolves stay read-mostly.
    pub(crate) async fn learn_identity_pairs(
        &mut self,
        db: &SessionStore,
        chat: &Chat,
        messages: &[ChatMessage],
    ) {
        if chat.is_group() {
            return;
        }

        let mut seen = HashSet::new();
        let chat_bare = bare_jid(&chat.jid);
        for message in messages {
            if message.outgoing || message.sender_jid.is_empty() {
                continue;
            }

            let bare = bare_jid(&message.sender_jid);
            if !seen.insert(bare.clone()) {
                continue;
            }

            let pair = if is_lid_jid(&chat_bare) && is_phone_jid(&bare) {
                Some((chat_bare.clone(), bare))
            } else if is_phone_jid(&chat_bare) && is_lid_jid(&bare) {
                Some((bare, chat_bare.clone()))
            } else {
                None
            };
            let Some((lid, phone)) = pair else {
                continue;
            };

            let lid_jid = format!("{}@lid", lid.split('@').next().unwrap_or_default());
            let phone_jid = format!(
                "{}@s.whatsapp.net",
                phone.split('@').next().unwrap_or_default()
            );
            if self.lid_to_pn.get(&lid_jid) == Some(&phone_jid) {
                continue;
            }

            if let Err(e) = db.learn_lid_mapping(&lid, &phone).await {
                tracing::debug!("Failed to learn identity pair {lid} -> {phone}: {e}");
            }

            self.lid_to_pn.insert(lid_jid.clone(), phone_jid.clone());
            self.pn_to_lids.entry(phone_jid).or_default().push(lid_jid);
            self.lid_digits_to_phone.insert(
                lid.split('@').next().unwrap_or_default().to_string(),
                phone.split('@').next().unwrap_or_default().to_string(),
            );
        }
    }

    /// Rebuilds one message's sender name from the snapshot, so
    /// contact saves and push-name updates made after the message
    /// was stored still show up, and old rows adopt push names for
    /// senders they never knew. Mirrors the app's live resolution
    /// order: real name, then the wire-sourced chain marked the
    /// `WhatsApp` way.
    pub(crate) fn resolve_message(&self, chat: &Chat, message: &mut ChatMessage) -> Option<String> {
        if message.outgoing || message.sender_jid.is_empty() {
            return None;
        }

        let bare = bare_jid(&message.sender_jid);
        let unknown = i18n!("Unknown");
        let chat_bare = bare_jid(&chat.jid);
        if chat.is_group() && bare == chat_bare {
            let stored = message
                .sender_name
                .take()
                .map(|name| name.trim_start_matches('~').to_string())
                .filter(|name| !looks_like_jid(name, &bare))?;
            let display = format!("~{stored}");
            message.sender_name = Some(display.clone());

            return Some(display);
        }

        let name = if let Some(name) =
            resolve_stored_name(&self.contacts, &self.lid_to_pn, &self.pn_to_lids, &bare)
        {
            Some(name)
        } else {
            // Wire-sourced fallbacks.
            let wire = chat
                .participants
                .get(&message.sender_jid)
                .or_else(|| chat.participants.get(&bare))
                .filter(|name| **name != unknown && !name.is_empty())
                .cloned()
                .or_else(|| {
                    message
                        .sender_name
                        .as_deref()
                        .map(|name| name.trim_start_matches('~'))
                        .filter(|name| !name.is_empty())
                        .map(ToString::to_string)
                });

            match wire {
                Some(name) if !looks_like_jid(&name, &bare) => Some(format!("~{name}")),
                // Junk "names" — ids posing as names in old rows or
                // participants entries — fall to the number
                // fallbacks, same as having no name at all.
                _ => {
                    if is_lid_jid(&bare) {
                        self.lid_to_pn.get(&bare).map(|pn| format_lid_as_number(pn))
                    } else if is_phone_jid(&bare) {
                        Some(format_lid_as_number(&bare))
                    } else {
                        None
                    }
                }
            }
        };

        if let Some(name) = name {
            message.sender_name = Some(name.clone());
            Some(name)
        } else {
            // Nothing resolves: keep the stored name untouched.
            None
        }
    }
}

/// Re-resolves every row's stored sender name against the current
/// snapshot and returns the digit-keyed LID identity map harvested
/// during the pass, for the timeline's author grouping. Empty chat
/// passes return an empty map.
pub(crate) async fn re_resolve_senders(
    db: &SessionStore,
    chat: &Chat,
    messages: &mut [ChatMessage],
) -> HashMap<String, String> {
    let mut seen = HashSet::new();
    let mut senders = Vec::new();
    for message in messages.iter() {
        if message.outgoing || message.sender_jid.is_empty() {
            continue;
        }

        let bare = bare_jid(&message.sender_jid);
        if seen.insert(bare.clone()) {
            senders.push(bare);
        }
    }

    let chat_bare = bare_jid(&chat.jid);
    let chat_is_group = chat.is_group();
    if chat_is_group {
        // Server-borne rows carry the group itself as their sender;
        // no contact will ever resolve for it.
        senders.retain(|sender| sender != &chat_bare);
    }

    let total = senders.len();
    if total == 0 {
        return HashMap::new();
    }

    let mut resolution = SenderResolution::new(db).await;
    resolution.learn_identity_pairs(db, chat, messages).await;
    let identities = resolution.identities();

    // Resolve every row, recording the first outcome per sender so
    // the stats reflect what the rows actually show, number
    // fallbacks included.
    let mut outcomes = HashMap::<String, Option<String>>::new();
    for message in messages.iter_mut() {
        let outcome = resolution.resolve_message(chat, message);
        if !message.outgoing && !message.sender_jid.is_empty() {
            outcomes
                .entry(bare_jid(&message.sender_jid))
                .or_insert(outcome);
        }
    }

    let mut named = 0;
    let mut push_sourced = 0;
    let mut unresolved_sample = Vec::new();
    for bare in &senders {
        match outcomes.get(bare) {
            Some(Some(name)) if name.starts_with('~') => {
                named += 1;
                push_sourced += 1;
            }
            Some(Some(_)) => named += 1,
            Some(None) | None if unresolved_sample.len() < 3 => {
                unresolved_sample.push(bare.clone());
            }
            Some(None) | None => {}
        }
    }
    tracing::debug!(
        "Re-resolved senders for {}: {total} seen, {named} named, {push_sourced} push-sourced, \
         {} unresolved ({unresolved_sample:?})",
        chat.jid,
        total - named,
    );

    identities
}

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
