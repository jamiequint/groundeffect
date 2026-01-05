//! Email data structures

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Attachment;

/// Email address with optional display name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Address {
    /// Display name (e.g., "John Doe")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Email address (e.g., "john@example.com")
    pub email: String,
}

impl Address {
    /// Create a new address with just an email
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            name: None,
            email: email.into(),
        }
    }

    /// Create a new address with name and email
    pub fn with_name(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    /// Format as "Name <email>" or just "email"
    pub fn to_string_full(&self) -> String {
        match &self.name {
            Some(name) => format!("{} <{}>", name, self.email),
            None => self.email.clone(),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{} <{}>", name, self.email),
            None => write!(f, "{}", self.email),
        }
    }
}

/// An email message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Email {
    // === Identifiers ===
    /// Internal UUID
    pub id: String,

    /// Account identifier (email address)
    pub account_id: String,

    /// User-defined account alias (e.g., "work", "personal")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_alias: Option<String>,

    /// RFC 5322 Message-ID
    pub message_id: String,

    /// Gmail X-GM-MSGID
    pub gmail_message_id: u64,

    /// Gmail X-GM-THRID (thread ID)
    pub gmail_thread_id: u64,

    /// IMAP UID
    pub uid: u32,

    // === Threading ===
    /// In-Reply-To header
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,

    /// References header
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,

    // === Metadata ===
    /// IMAP folder
    pub folder: String,

    /// Gmail labels
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,

    /// IMAP flags (Seen, Flagged, etc.)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,

    // === Headers ===
    /// From address
    pub from: Address,

    /// To addresses
    #[serde(default)]
    pub to: Vec<Address>,

    /// CC addresses
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Address>,

    /// BCC addresses
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<Address>,

    /// Subject line
    pub subject: String,

    /// Date sent
    pub date: DateTime<Utc>,

    // === Content ===
    /// Plain text body
    pub body_plain: String,

    /// HTML body (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,

    /// Preview snippet (first ~200 chars)
    pub snippet: String,

    // === Attachments ===
    /// List of attachments
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,

    // === Search ===
    /// Embedding vector (768 dimensions)
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,

    // === Sync metadata ===
    /// When this email was last synced
    pub synced_at: DateTime<Utc>,

    /// Raw message size in bytes
    pub raw_size: u64,
}

impl Email {
    /// Check if the email has been read
    pub fn is_read(&self) -> bool {
        self.flags.iter().any(|f| f == "\\Seen")
    }

    /// Check if the email is flagged/starred
    pub fn is_flagged(&self) -> bool {
        self.flags.iter().any(|f| f == "\\Flagged")
    }

    /// Check if the email has attachments
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }

    /// Get searchable text for embedding
    pub fn searchable_text(&self) -> String {
        let mut text = String::new();

        // Subject (weighted by repetition for importance)
        text.push_str(&self.subject);
        text.push_str(". ");
        text.push_str(&self.subject);
        text.push_str(". ");

        // Sender
        text.push_str("From: ");
        text.push_str(&self.from.to_string_full());
        text.push_str(". ");

        // Body
        text.push_str(&self.body_plain);

        // Attachment filenames
        if !self.attachments.is_empty() {
            text.push_str(" Attachments: ");
            for att in &self.attachments {
                text.push_str(&att.filename);
                text.push(' ');
            }
        }

        text
    }

    /// Generate a markdown summary
    pub fn markdown_summary(&self) -> String {
        let account_display = match &self.account_alias {
            Some(alias) => format!("{} ({})", self.account_id, alias),
            None => self.account_id.clone(),
        };

        format!(
            "**Account:** {}\n**From:** {}\n**Subject:** {}\n**Date:** {}\n\n{}",
            account_display,
            self.from,
            self.subject,
            self.date.format("%b %d, %Y %I:%M %p"),
            self.snippet
        )
    }
}

/// Email search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSearchResult {
    /// The email data
    #[serde(flatten)]
    pub email: EmailSummary,

    /// Combined search score (RRF)
    pub score: f32,

    /// Markdown summary for LLM consumption
    pub markdown_summary: String,
}

/// Lightweight attachment info for search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentSummary {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_human: String,
    pub downloaded: bool,
}

/// Lightweight email summary for search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSummary {
    pub id: String,
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_alias: Option<String>,
    pub message_id: String,
    pub thread_id: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub subject: String,
    pub date: DateTime<Utc>,
    pub snippet: String,
    pub has_attachments: bool,
    pub attachments: Vec<AttachmentSummary>,
    pub labels: Vec<String>,
}

impl From<&Email> for EmailSummary {
    fn from(email: &Email) -> Self {
        Self {
            id: email.id.clone(),
            account_id: email.account_id.clone(),
            account_alias: email.account_alias.clone(),
            message_id: email.message_id.clone(),
            thread_id: email.gmail_thread_id.to_string(),
            from: email.from.clone(),
            to: email.to.clone(),
            subject: email.subject.clone(),
            date: email.date,
            snippet: email.snippet.clone(),
            has_attachments: !email.attachments.is_empty(),
            attachments: email.attachments.iter().map(|a| AttachmentSummary {
                id: a.id.clone(),
                filename: a.filename.clone(),
                mime_type: a.mime_type.clone(),
                size_human: a.size_human(),
                downloaded: a.downloaded,
            }).collect(),
            labels: email.labels.clone(),
        }
    }
}

/// Request to send an email
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendEmailRequest {
    /// Account to send from (email or alias)
    pub from_account: String,

    /// Recipient email addresses
    pub to: Vec<String>,

    /// Email subject
    pub subject: String,

    /// Plain text body
    pub body: String,

    /// CC recipients
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,

    /// BCC recipients
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,

    /// Local file paths to attach
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,

    /// Message-ID to reply to (for threading)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
}
