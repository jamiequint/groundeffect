//! Attachment data structures

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// An email attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Unique attachment ID
    pub id: String,

    /// Original filename
    pub filename: String,

    /// MIME type
    pub mime_type: String,

    /// Size in bytes
    pub size: u64,

    /// Local file path (if downloaded)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<PathBuf>,

    /// Content-ID for inline attachments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,

    /// Whether the attachment has been downloaded
    #[serde(default)]
    pub downloaded: bool,
}

impl Attachment {
    /// Create a new attachment (metadata only, not downloaded)
    pub fn new(
        id: impl Into<String>,
        filename: impl Into<String>,
        mime_type: impl Into<String>,
        size: u64,
    ) -> Self {
        Self {
            id: id.into(),
            filename: filename.into(),
            mime_type: mime_type.into(),
            size,
            local_path: None,
            content_id: None,
            downloaded: false,
        }
    }

    /// Check if this is an inline attachment
    pub fn is_inline(&self) -> bool {
        self.content_id.is_some()
    }

    /// Check if this attachment is an image
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    /// Check if this attachment is a PDF
    pub fn is_pdf(&self) -> bool {
        self.mime_type == "application/pdf"
    }

    /// Get human-readable size
    pub fn size_human(&self) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if self.size >= GB {
            format!("{:.1} GB", self.size as f64 / GB as f64)
        } else if self.size >= MB {
            format!("{:.1} MB", self.size as f64 / MB as f64)
        } else if self.size >= KB {
            format!("{:.1} KB", self.size as f64 / KB as f64)
        } else {
            format!("{} bytes", self.size)
        }
    }
}
