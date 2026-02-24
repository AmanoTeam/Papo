use adw::gtk;
use glib::Bytes;
use glycin::Loader;
use gtk::{gdk, glib};
use image::{ExtendedColorType, ImageEncoder, Luma, codecs::png::PngEncoder};
use qrcode::QrCode;
use rlibphonenumber::{PhoneNumber, PhoneNumberFormat};

/// Get only the first name from a full name.
pub fn get_first_name(name: &str) -> String {
    if name.is_empty() {
        String::new()
    } else if name.contains(' ') {
        let (first, _) = name.split_once(' ').unwrap();

        first.chars().next().unwrap().to_string()
    } else {
        name.to_string()
    }
}

/// Generate a QR code texture.
pub async fn generate_qr_code(data: &str) -> Result<gdk::Texture, Box<dyn std::error::Error>> {
    let qr_code = QrCode::new(data.as_bytes())?;
    let image = qr_code.render::<Luma<u8>>().build();

    // Encode the QR code as a PNG.
    let mut bytes = Vec::new();
    let encoder = PngEncoder::new(&mut bytes);
    encoder.write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::L8,
    )?;

    // Load the image through glycin.
    let loader = Loader::new_bytes(Bytes::from_owned(bytes));
    let image = loader.load().await?;
    let frame = image.next_frame().await?;
    let texture = frame.texture();

    Ok(texture)
}

/// Format a LID as international phone number.
pub fn format_lid_as_number(lid: &str) -> String {
    let phone = extract_phone_from_jid(lid);

    phone.parse::<PhoneNumber>().map_or(phone, |number| {
        number
            .format_as(PhoneNumberFormat::International)
            .to_string()
    })
}

/// Extract phone number from JID/LID.
pub fn extract_phone_from_jid(jid: &str) -> String {
    format!("+{}", jid.split('@').next().unwrap_or(jid))
}
