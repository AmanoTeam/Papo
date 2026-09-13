use std::sync::Arc;

use whatsapp_rust::download::MediaType as DownloadMediaType;

use crate::i18n;

#[derive(Clone, Debug, Default)]
pub struct Media {
    pub data: Arc<Vec<u8>>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r#type: MediaType,
    pub caption: Option<String>,
    pub animated: bool,
    pub mime_type: String,
    /// Download info for fetching full media (videos, documents).
    pub downloadable: Option<DownloadableMedia>,
    /// Duration in seconds (for audio/video).
    pub duration_secs: Option<u32>,
}

impl Media {
    pub fn can_play(&self) -> bool {
        self.has_data() || self.can_download()
    }

    /// Checks if this media has inline data available.
    pub fn has_data(&self) -> bool {
        !self.data.is_empty()
    }

    pub fn can_download(&self) -> bool {
        self.downloadable.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum MediaType {
    Audio,
    /// Image (JPEG, PNG, WebP).
    #[default]
    Image,
    Video,
    /// Sticker (WebP, animated or static)
    Sticker,
    Document,
}

impl MediaType {
    pub fn display_label(self) -> String {
        match self {
            Self::Audio => format!("🎤 {}", i18n!("Voice message")),
            Self::Image => format!("📷 {}", i18n!("Photo")),
            Self::Video => format!("🎥 {}", i18n!("Video")),
            Self::Sticker => format!("🎭 {}", i18n!("Sticker")),
            Self::Document => format!("📄 {}", i18n!("Document")),
        }
    }

    pub fn guess_mime_type(self) -> String {
        match self {
            Self::Audio => "audio/ogg".to_string(),
            Self::Image => "image/jpeg".to_string(),
            Self::Video => "video/mp4".to_string(),
            Self::Sticker => "image/webp".to_string(),
            Self::Document => "application/pdf".to_string(),
        }
    }
}

impl From<&str> for MediaType {
    fn from(value: &str) -> Self {
        match value.to_lowercase().as_str() {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "sticker" => Self::Sticker,
            "document" => Self::Document,
            _ => Self::Image,
        }
    }
}

impl From<String> for MediaType {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// Information needed to download encrypted media from `WhatsApp` servers.
/// This is stored separately from the thumbnail/preview data.
#[derive(Clone, Debug)]
pub struct DownloadableMedia {
    pub media_key: Vec<u8>,
    /// MIME type of the actual media (e.g., "video/mp4").
    pub mime_type: String,
    /// Direct path for CDN URL construction.
    pub direct_path: String,
    pub file_length: u64,
    /// Download media type (for key derivation).
    pub download_type: DownloadMediaType,
    /// Duration in seconds (for video/audio).
    pub duration_secs: Option<u32>,
    /// SHA256 of encrypted file (used for URL token).
    pub file_enc_sha256: Vec<u8>,
}
