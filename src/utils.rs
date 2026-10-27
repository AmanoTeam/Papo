use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    error::Error,
};

use fast_qr::{
    ECL, QRBuilder,
    convert::{Builder, Shape, image::ImageBuilder},
};
use glib::Bytes;
use glycin::Loader;
use gtk::{gdk, glib};
use jiff::{Zoned, civil::Date};
use relm4::gtk;
use rlibphonenumber::{PhoneNumber, PhoneNumberFormat};

use crate::{
    db::{entities::Contact, store::SessionStore},
    i18n,
    state::{Chat, ChatMessage},
};

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

/// Truncates `text` to at most `max_chars` characters, appending `…`
/// when the cap was reached.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

/// Truncates `text` to at most `max_lines` whole lines, clipping each
/// line to about `max_line_chars` characters of visible width, with
/// the `…` marker applied whenever content was lost. Long
/// single-paragraph quotes get cut at the approximate row width; multi-
/// paragraph quotes preserve their embedded line breaks.
pub fn truncate_lines(text: &str, max_lines: usize, max_line_chars: usize) -> String {
    let mut kept = Vec::new();
    let mut lost = false;

    for (index, line) in text.lines().enumerate() {
        if index == max_lines {
            lost = true;
            break;
        }

        if line.chars().count() > max_line_chars {
            lost = true;
            kept.push(truncate_chars(line, max_line_chars));
        } else {
            kept.push(line.to_string());
        }
    }

    let mut result = kept.join("\n");
    if lost && !result.ends_with('…') {
        result.push('…');
    }
    result
}

/// Whether the jid addresses a contact by its linked-device identity.
pub fn is_lid_jid(jid: &str) -> bool {
    jid.ends_with("@lid")
}

/// Whether the jid addresses a contact by their phone number.
pub fn is_phone_jid(jid: &str) -> bool {
    jid.ends_with("@s.whatsapp.net") || jid.ends_with("@c.us")
}

/// Whether the jid addresses a group chat.
pub fn is_group_jid(jid: &str) -> bool {
    jid.ends_with("@g.us")
}

/// Whether the jid belongs to a broadcast conversation: statuses and
/// broadcast lists share the domain.
pub fn is_broadcast_jid(jid: &str) -> bool {
    jid.ends_with("@broadcast")
}

/// Orders chats for display: the pinned block first, pins sorted by pin
/// time with the NEWEST pin on top and unpinned chats by newest activity.
/// Every list sort and insert shares this one rule.
pub fn cmp_chat_order<T: Ord>(a: &(bool, Option<i64>, T), b: &(bool, Option<i64>, T)) -> Ordering {
    match (a.0, b.0) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => b.1.unwrap_or(i64::MIN).cmp(&a.1.unwrap_or(i64::MIN)),
        (false, false) => b.2.cmp(&a.2),
    }
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
    /// Every identity the account renders as itself (`You`): its own
    /// phone form plus every LID form the phone uses.
    own_forms: Vec<String>,
    lid_to_pn: HashMap<String, String>,
    pn_to_lids: HashMap<String, Vec<String>>,
    /// Digit-keyed index of the same pairs, handed to the timeline
    /// so one person's LID-keyed and phone-numbered rows group as
    /// one author.
    lid_digits_to_phone: HashMap<String, String>,
}

impl SenderResolution {
    pub(crate) async fn new(db: &SessionStore, own_forms: Vec<String>) -> Self {
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
            own_forms,
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
        // The quoted author is anonymous to the wire: resolve like any
        // other sender, with the number fallback; the display survives
        // across rebinds via the stored name.
        if let Some(quote) = &mut message.quote {
            quote.sender_name = quote.participant.as_deref().and_then(|bare| {
                if !self.own_forms.is_empty() && self.own_forms.contains(&bare.to_string()) {
                    return Some(i18n!("You"));
                }

                let resolved =
                    resolve_stored_name(&self.contacts, &self.lid_to_pn, &self.pn_to_lids, bare)
                        .or_else(|| {
                            if is_lid_jid(bare) {
                                self.lid_to_pn.get(bare).map(|pn| format_lid_as_number(pn))
                            } else if is_phone_jid(bare) {
                                Some(format_lid_as_number(bare))
                            } else {
                                None
                            }
                        });
                if resolved.is_none() {
                    tracing::debug!(
                        "Quote author {bare} unresolved: contact={} pn-mapping={}",
                        self.contacts.contains_key(bare),
                        is_lid_jid(bare).then(|| self.lid_to_pn.get(bare)).is_some()
                    );
                }

                resolved
            });
        }

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
    own_forms: &[String],
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

    let mut resolution = SenderResolution::new(db, own_forms.to_vec()).await;
    resolution.learn_identity_pairs(db, chat, messages).await;
    let identities = resolution.identities();

    // Resolve every row, recording the first outcome per sender so
    // the stats reflect what the rows actually show, number
    // fallbacks included. Rows whose resolution beats what was saved
    // get persisted back at once — the stored name stops being the
    // pre-contact rendering it was ingested with.
    let mut outcomes = HashMap::<String, Option<String>>::new();
    for message in messages.iter_mut() {
        let sender_before = message.sender_name.clone();
        let quote_before = message
            .quote
            .as_ref()
            .and_then(|quote| quote.sender_name.clone());

        let outcome = resolution.resolve_message(chat, message);
        if !message.outgoing && !message.sender_jid.is_empty() {
            outcomes
                .entry(bare_jid(&message.sender_jid))
                .or_insert(outcome);
        }

        let quote_after = message
            .quote
            .as_ref()
            .and_then(|quote| quote.sender_name.clone());
        let sender_changed = message.sender_name != sender_before;
        let quote_changed = quote_after != quote_before;
        if sender_changed || quote_changed {
            let local_id = message.local_id.to_string();
            if let Err(e) = db
                .update_message_names(
                    &local_id,
                    message.sender_name.as_deref(),
                    quote_after.as_deref(),
                )
                .await
            {
                tracing::debug!("Failed to write back resolved sender names for {local_id}: {e}");
            }
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
    tracing::trace!(
        "Re-resolved senders for {}: {total} seen, {named} named, {push_sourced} push-sourced, \
         {} unresolved ({unresolved_sample:?})",
        chat.jid,
        total - named,
    );

    identities
}

#[cfg(test)]
mod tests {
    use super::truncate_lines;

    #[test]
    fn truncate_lines_keeps_whole_lines() {
        assert_eq!(truncate_lines("one", 2, 85), "one");
        assert_eq!(truncate_lines("one\ntwo", 2, 85), "one\ntwo");
        assert_eq!(truncate_lines("one\ntwo\nthree", 2, 85), "one\ntwo…");
        assert_eq!(truncate_lines("", 2, 85), "");
    }

    #[test]
    fn truncate_lines_clips_per_line_width() {
        let long = "abcdefghijklmno";

        assert_eq!(truncate_lines(long, 1, 6), "abcdef…");
        assert_eq!(
            truncate_lines("one\ntwo\nthree\nfour\nfive\nsix\nseven", 5, 85),
            "one\ntwo\nthree\nfour\nfive…"
        );
    }
}
