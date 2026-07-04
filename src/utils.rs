use chrono::{Datelike, Local, NaiveDate};
use fast_qr::{
    ECL, QRBuilder,
    convert::{Builder, Shape, image::ImageBuilder},
};
use glib::Bytes;
use glycin::Loader;
use gtk::{gdk, glib};
use relm4::prelude::*;
use rlibphonenumber::{PhoneNumber, PhoneNumberFormat};

use crate::i18n;

/// Gets only the first name from a full name.
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

/// Generates a QR code texture.
pub async fn generate_qr_code(
    data: &str,
    size: u32,
) -> Result<gdk::Texture, Box<dyn std::error::Error>> {
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
    let image_doc = loader.load().await?;
    let frame = image_doc.next_frame().await?;
    let texture = frame.texture();

    Ok(texture)
}

/// Formats a date into a human-readable label for date separators.
pub fn format_date_label(date: NaiveDate) -> String {
    let today = Local::now().date_naive();

    if date == today {
        return i18n!("Today");
    }

    if let Some(yesterday) = today.pred_opt()
        && date == yesterday
    {
        return i18n!("Yesterday");
    }

    // Same year: "February 23", different year: "February 23, 2024".
    if date.year() == today.year() {
        date.format("%B %-e").to_string()
    } else {
        date.format("%B %-e, %Y").to_string()
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

/// Extracts phone number from JID/LID.
pub fn extract_phone_from_jid(jid: &str) -> String {
    format!("+{}", jid.split('@').next().unwrap_or(jid))
}
