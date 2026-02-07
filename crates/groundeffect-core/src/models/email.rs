//! Email data structures

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::Attachment;

const SEARCHABLE_BODY_MAX_CHARS: usize = 16_000;
const SEARCHABLE_BODY_TAIL_CHARS: usize = 2_000;
const HTML2TEXT_FALLBACK_WIDTH: usize = 100;

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
    /// Body content used for search/display:
    /// plain text when present, otherwise markdown converted from HTML.
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
    fn sanitize_body_text(text: &str) -> String {
        text.chars()
            .filter(|c| !matches!(c, '\u{0000}'..='\u{0008}' | '\u{000B}' | '\u{000C}' | '\u{000E}'..='\u{001F}' | '\u{007F}'))
            .collect()
    }

    fn html_to_markdown_with_fallback(html: &str) -> String {
        let sanitized_html = Self::sanitize_body_text(html);
        if sanitized_html.trim().is_empty() {
            return String::new();
        }

        match html_to_markdown_rs::convert(&sanitized_html, None) {
            Ok(markdown) => {
                let sanitized_markdown = Self::sanitize_body_text(&markdown);
                if !sanitized_markdown.trim().is_empty() {
                    return sanitized_markdown;
                }
            }
            Err(err) => {
                warn!(
                    "HTML->Markdown conversion failed ({} chars), falling back to html2text: {}",
                    sanitized_html.chars().count(),
                    err
                );
            }
        }

        let fallback = html2text::from_read(sanitized_html.as_bytes(), HTML2TEXT_FALLBACK_WIDTH)
            .unwrap_or_default();
        Self::sanitize_body_text(&fallback)
    }

    /// Resolve the canonical body text used for embeddings, BM25, and user display.
    /// Priority: plaintext body first, otherwise HTML converted to markdown.
    pub fn body_for_indexing_and_display(body_plain: &str, body_html: Option<&str>) -> String {
        let plain = Self::sanitize_body_text(body_plain);
        if !plain.trim().is_empty() {
            return plain;
        }

        match body_html {
            Some(html) if !html.trim().is_empty() => Self::html_to_markdown_with_fallback(html),
            _ => String::new(),
        }
    }

    /// Resolve this email's canonical display/search body text.
    pub fn resolved_body(&self) -> String {
        Self::body_for_indexing_and_display(&self.body_plain, self.body_html.as_deref())
    }

    fn embedding_body_excerpt(body: &str) -> String {
        let total_chars = body.chars().count();
        if total_chars <= SEARCHABLE_BODY_MAX_CHARS {
            return body.to_string();
        }

        let tail_chars = SEARCHABLE_BODY_TAIL_CHARS.min(total_chars);
        let head_chars = SEARCHABLE_BODY_MAX_CHARS
            .saturating_sub(tail_chars)
            .saturating_sub(32);

        let head: String = body.chars().take(head_chars).collect();
        let tail: String = body
            .chars()
            .skip(total_chars.saturating_sub(tail_chars))
            .collect();

        format!("{} [truncated] {}", head, tail)
    }

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
        let body = self.resolved_body();
        text.push_str(&Self::embedding_body_excerpt(&body));

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
            attachments: email
                .attachments
                .iter()
                .map(|a| AttachmentSummary {
                    id: a.id.clone(),
                    filename: a.filename.clone(),
                    mime_type: a.mime_type.clone(),
                    size_human: a.size_human(),
                    downloaded: a.downloaded,
                })
                .collect(),
            labels: email.labels.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Email, SEARCHABLE_BODY_MAX_CHARS};

    #[test]
    fn embedding_body_excerpt_keeps_short_body_unchanged() {
        let body = "short body";
        assert_eq!(Email::embedding_body_excerpt(body), body);
    }

    #[test]
    fn embedding_body_excerpt_truncates_long_body() {
        let long = "a".repeat(SEARCHABLE_BODY_MAX_CHARS + 5000);
        let excerpt = Email::embedding_body_excerpt(&long);
        assert!(excerpt.chars().count() <= SEARCHABLE_BODY_MAX_CHARS);
        assert!(excerpt.contains("[truncated]"));
    }

    #[test]
    fn body_resolution_prefers_plain_text() {
        let body = Email::body_for_indexing_and_display(
            "Plain body",
            Some("<p>HTML body should not win</p>"),
        );
        assert_eq!(body, "Plain body");
    }

    #[test]
    fn body_resolution_uses_markdown_from_html_when_plain_missing() {
        let html = "<h1>Hello</h1><p>See <a href=\"https://example.com\">example</a></p>";
        let body = Email::body_for_indexing_and_display("", Some(html));
        assert!(body.contains("Hello"));
        assert!(body.contains("https://example.com"));
        assert!(!body.contains("<h1>"));
    }

    #[test]
    fn body_resolution_strips_control_chars() {
        let body = Email::body_for_indexing_and_display("hello\u{0000}\u{0007}world", None);
        assert_eq!(body, "helloworld");
    }

    #[test]
    fn body_resolution_handles_html_with_control_chars() {
        let html = "<p>hello\u{0000}\u{0007}<a href=\"https://example.com\">world</a></p>";
        let body = Email::body_for_indexing_and_display("", Some(html));
        assert!(body.contains("hello"));
        assert!(body.contains("https://example.com"));
        assert!(!body.contains('\u{0000}'));
        assert!(!body.contains('\u{0007}'));
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
