//! MCP tool implementations

use std::process::Command;
use std::sync::Arc;

use chrono::{DateTime, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use super::protocol::{ToolDefinition, ToolResult};
use crate::config::{Config, DaemonConfig};
use crate::db::Database;
use crate::error::{Error, Result};
use crate::models::{Account, AccountStatus, Email, SendEmailRequest};
use crate::oauth::OAuthManager;
use crate::search::{CalendarSearchOptions, SearchEngine, SearchOptions};

/// Get all tool definitions
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        // Account management
        ToolDefinition {
            name: "manage_accounts".to_string(),
            description: "Manage Gmail/GCal accounts. Actions: 'list' (all accounts), 'get' (one account), 'add' (OAuth flow), 'delete' (remove account+data), 'configure' (update settings).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "get", "add", "delete", "configure"],
                        "description": "Action: 'list' (all accounts), 'get' (one account), 'add' (OAuth), 'delete' (remove), 'configure' (settings)"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account email or alias. Required for get/delete/configure."
                    },
                    "alias": {
                        "type": "string",
                        "description": "For 'add': friendly name. For 'configure': new alias (or null to remove)."
                    },
                    "years_to_sync": {
                        "type": "string",
                        "description": "For 'add': years of email history to sync ('1'-'20' or 'all'). REQUIRED - will prompt if not provided."
                    },
                    "sync_email": {
                        "type": "boolean",
                        "description": "For 'configure': enable/disable email sync"
                    },
                    "sync_calendar": {
                        "type": "boolean",
                        "description": "For 'configure': enable/disable calendar sync"
                    },
                    "folders": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "For 'configure': folders to sync (empty array = all folders)"
                    },
                    "sync_attachments": {
                        "type": "boolean",
                        "description": "For 'configure': enable/disable automatic attachment download during sync (off by default, requires daemon restart)"
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "For 'delete': must be true to confirm deletion"
                    }
                },
                "required": ["action"]
            }),
        },
        // Email tools
        ToolDefinition {
            name: "search_emails".to_string(),
            description: "Search emails using hybrid BM25 + vector search across one or more accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (natural language)"
                    },
                    "intent": {
                        "type": "string",
                        "enum": ["search", "list"],
                        "description": "Use 'list' for recent/latest/unread requests (fast path), 'search' for content-based queries (semantic search)"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Account(s) to search (email addresses or aliases). Omit to search ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100
                    },
                    "folder": {
                        "type": "string",
                        "description": "Filter by folder (e.g., INBOX, Sent)"
                    },
                    "from": {
                        "type": "string",
                        "description": "Filter by sender email/name"
                    },
                    "to": {
                        "type": "string",
                        "description": "Filter by recipient email/name"
                    },
                    "date_from": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter emails after this date"
                    },
                    "date_to": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter emails before this date"
                    },
                    "has_attachment": {
                        "type": "boolean",
                        "description": "Filter emails with attachments"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "list_emails".to_string(),
            description: "List recent emails sorted by date (newest first). Much faster than search_emails for just getting recent messages.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias. Omit to list from ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100,
                        "description": "Number of emails to return"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "get_email".to_string(),
            description: "Fetch single email by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Email ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "get_thread".to_string(),
            description: "Fetch all emails in a thread".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "thread_id": {
                        "type": "string",
                        "description": "Gmail thread ID"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                },
                "required": ["thread_id"]
            }),
        },
        ToolDefinition {
            name: "send_email".to_string(),
            description: "Compose and send an email. By default returns a preview for user confirmation. Set confirm=true to send, or save_as_draft=true to save as draft. Supports HTML via explicit flag or auto-detection of markdown links, plain URLs, bold/italic markdown, or HTML tags.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from_account": {
                        "type": "string",
                        "description": "Account email or alias to send from"
                    },
                    "to": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Recipient email addresses"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Email subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Email body (plain text, markdown, or HTML)"
                    },
                    "cc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "CC recipients"
                    },
                    "bcc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "BCC recipients"
                    },
                    "reply_to_id": {
                        "type": "string",
                        "description": "Email ID to reply to (for threading)"
                    },
                    "html": {
                        "type": "boolean",
                        "description": "Force HTML format. If false, auto-detects based on content (markdown links, URLs, bold/italic, HTML tags)."
                    },
                    "save_as_draft": {
                        "type": "boolean",
                        "description": "Save as draft instead of sending. Returns draft_id. Use send_draft to send later."
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "Set to true to send immediately. If false/omitted, returns preview for user approval.",
                        "default": false
                    }
                },
                "required": ["from_account", "to", "subject", "body"]
            }),
        },
        ToolDefinition {
            name: "list_folders".to_string(),
            description: "List all IMAP folders".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "get_attachment".to_string(),
            description: "Get an email attachment. Returns content for text files, file path for binary files (use Read tool on path).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "email_id": {
                        "type": "string",
                        "description": "Email ID containing the attachment"
                    },
                    "attachment_id": {
                        "type": "string",
                        "description": "Attachment ID (from get_email response)"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Attachment filename (alternative to attachment_id)"
                    }
                },
                "required": ["email_id"]
            }),
        },
        // Draft tools
        ToolDefinition {
            name: "create_draft".to_string(),
            description: "Create a new email draft directly (no preview/confirm flow). Returns draft_id that can be used with send_draft, get_draft, update_draft, or delete_draft.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from_account": {
                        "type": "string",
                        "description": "Account email or alias to create draft from"
                    },
                    "to": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Recipient email addresses"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Email subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Email body (plain text, markdown, or HTML)"
                    },
                    "cc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "CC recipients"
                    },
                    "bcc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "BCC recipients"
                    },
                    "html": {
                        "type": "boolean",
                        "description": "Force HTML format. If false, auto-detects based on content."
                    },
                    "reply_to_id": {
                        "type": "string",
                        "description": "Email ID to reply to (for threading)"
                    }
                },
                "required": ["from_account", "to", "subject", "body"]
            }),
        },
        ToolDefinition {
            name: "list_drafts".to_string(),
            description: "List all email drafts for an account. Drafts are fetched directly from Gmail API (not stored locally).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 20,
                        "maximum": 100,
                        "description": "Maximum number of drafts to return"
                    }
                },
                "required": ["account"]
            }),
        },
        ToolDefinition {
            name: "get_draft".to_string(),
            description: "Get full content of a specific draft by ID.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    },
                    "draft_id": {
                        "type": "string",
                        "description": "Draft ID from list_drafts"
                    }
                },
                "required": ["account", "draft_id"]
            }),
        },
        ToolDefinition {
            name: "update_draft".to_string(),
            description: "Update an existing draft. Only provided fields are updated; omitted fields keep their current values.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    },
                    "draft_id": {
                        "type": "string",
                        "description": "Draft ID to update"
                    },
                    "to": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "New recipient email addresses"
                    },
                    "subject": {
                        "type": "string",
                        "description": "New subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "New email body"
                    },
                    "cc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "New CC recipients"
                    },
                    "bcc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "New BCC recipients"
                    },
                    "html": {
                        "type": "boolean",
                        "description": "Force HTML format"
                    }
                },
                "required": ["account", "draft_id"]
            }),
        },
        ToolDefinition {
            name: "send_draft".to_string(),
            description: "Send an existing draft by ID.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    },
                    "draft_id": {
                        "type": "string",
                        "description": "Draft ID to send"
                    }
                },
                "required": ["account", "draft_id"]
            }),
        },
        ToolDefinition {
            name: "delete_draft".to_string(),
            description: "Delete an existing draft by ID.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    },
                    "draft_id": {
                        "type": "string",
                        "description": "Draft ID to delete"
                    }
                },
                "required": ["account", "draft_id"]
            }),
        },
        // Calendar tools
        ToolDefinition {
            name: "search_events".to_string(),
            description: "Search calendar events across one or more accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (natural language)"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Account(s) to search. Omit to search ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100
                    },
                    "calendar_id": {
                        "type": "string",
                        "description": "Filter to specific calendar"
                    },
                    "date_from": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter events after this date"
                    },
                    "date_to": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter events before this date"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_event".to_string(),
            description: "Fetch single calendar event by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Event ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "list_events".to_string(),
            description: "List calendar events in a date range WITHOUT semantic search. Use this to answer questions like 'what's on my calendar tomorrow' or 'show me my meetings next week'. Unlike search_events, this does NOT require a search query - it simply lists all events in the specified date range chronologically.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "format": "date",
                        "description": "Start date (YYYY-MM-DD). Defaults to today if omitted."
                    },
                    "to": {
                        "type": "string",
                        "format": "date",
                        "description": "End date (YYYY-MM-DD). Defaults to 7 days after 'from' if omitted."
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts (email or alias). Omit to list from ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 50,
                        "maximum": 200,
                        "description": "Maximum number of events to return"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "list_calendars".to_string(),
            description: "List all calendars".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "create_event".to_string(),
            description: "Create a new calendar event".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account to create event on (email or alias)"
                    },
                    "summary": {
                        "type": "string",
                        "description": "Event title"
                    },
                    "start": {
                        "type": "string",
                        "format": "date-time",
                        "description": "Start time (ISO 8601)"
                    },
                    "end": {
                        "type": "string",
                        "format": "date-time",
                        "description": "End time (ISO 8601)"
                    },
                    "calendar_id": {
                        "type": "string",
                        "description": "Calendar ID (omit for primary)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Event description"
                    },
                    "location": {
                        "type": "string",
                        "description": "Event location"
                    },
                    "attendees": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Attendee email addresses"
                    }
                },
                "required": ["account", "summary", "start", "end"]
            }),
        },
        // System tools
        ToolDefinition {
            name: "manage_sync".to_string(),
            description: "Manage sync. Actions: 'status' (show sync status), 'reset' (clear synced data), 'extend' (sync older data), 'resume_from' (force resume from date), 'download_attachments' (download pending attachments).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "reset", "extend", "resume_from", "download_attachments"],
                        "description": "Action: 'status' (show sync info), 'reset' (clear data), 'extend' (sync older data), 'resume_from' (force sync from date), 'download_attachments' (retroactively download attachments for existing emails)"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account email or alias. Optional for 'status' (shows all accounts if omitted). Required for other actions."
                    },
                    "target_date": {
                        "type": "string",
                        "format": "date",
                        "description": "For 'extend' or 'resume_from': Date in YYYY-MM-DD format"
                    },
                    "data_type": {
                        "type": "string",
                        "enum": ["email", "calendar", "all"],
                        "description": "For 'reset': What data to reset (default: all)"
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "For 'reset': Must be true to confirm deletion"
                    }
                },
                "required": ["action"]
            }),
        },
        // Daemon management tools
        ToolDefinition {
            name: "manage_daemon".to_string(),
            description: "Manage the GroundEffect sync daemon. Can start, stop, restart, or check status of the daemon that syncs emails and calendar events in the background.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["start", "stop", "restart", "status"],
                        "description": "Action to perform: 'start', 'stop', 'restart', or 'status'"
                    },
                    "logging": {
                        "type": "boolean",
                        "description": "Enable logging to ~/.local/share/groundeffect/logs/ (for start/restart)"
                    },
                    "email_poll_interval": {
                        "type": "integer",
                        "description": "Email poll interval in seconds (for start/restart, default: 300)"
                    },
                    "calendar_poll_interval": {
                        "type": "integer",
                        "description": "Calendar poll interval in seconds (for start/restart, default: 300)"
                    },
                    "max_concurrent_fetches": {
                        "type": "integer",
                        "description": "Max concurrent email fetches (for start/restart, default: 10)"
                    }
                },
                "required": ["action"]
            }),
        },
    ]
}

// ============================================================================
// Email Helper Functions
// ============================================================================

/// Detect if body content should be treated as HTML
/// Triggers on: markdown links [text](url), plain URLs, bold **text**, italic *text*, HTML tags
fn detect_html_content(body: &str) -> bool {
    use regex::Regex;

    // Check for markdown links: [text](url)
    let md_link = Regex::new(r"\[.+?\]\(.+?\)").unwrap();
    if md_link.is_match(body) {
        return true;
    }

    // Check for plain URLs (http:// or https://)
    let url_pattern = Regex::new(r"https?://[^\s]+").unwrap();
    if url_pattern.is_match(body) {
        return true;
    }

    // Check for bold: **text** or __text__
    let bold = Regex::new(r"\*\*.+?\*\*|__.+?__").unwrap();
    if bold.is_match(body) {
        return true;
    }

    // Check for italic: *text* or _text_ (single asterisks not part of bold)
    let italic = Regex::new(r"(?:^|[^*])\*[^*\n]+?\*(?:[^*]|$)").unwrap();
    if italic.is_match(body) {
        return true;
    }

    // Check for HTML tags: <tag>, <tag attr="value">, </tag>
    let html_tag = Regex::new(r"</?[a-zA-Z][^>]*>").unwrap();
    if html_tag.is_match(body) {
        return true;
    }

    false
}

/// Convert markdown/plain text to HTML
fn convert_to_html(body: &str) -> String {
    use regex::Regex;

    let mut html = body.to_string();

    // Convert markdown links [text](url) to <a href="url">text</a>
    let md_link = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
    html = md_link
        .replace_all(&html, r#"<a href="$2">$1</a>"#)
        .to_string();

    // Convert plain URLs to links (not if preceded by " or > which means already in href)
    let url_pattern = Regex::new(r#"(^|[^">])(https?://[^\s<>"]+)"#).unwrap();
    html = url_pattern
        .replace_all(&html, r#"$1<a href="$2">$2</a>"#)
        .to_string();

    // Convert **bold** to <strong>
    let bold = Regex::new(r"\*\*(.+?)\*\*").unwrap();
    html = bold.replace_all(&html, r"<strong>$1</strong>").to_string();

    // Convert __bold__ to <strong>
    let bold2 = Regex::new(r"__(.+?)__").unwrap();
    html = bold2.replace_all(&html, r"<strong>$1</strong>").to_string();

    // Convert *italic* to <em> - safe now since ** is already converted to <strong>
    let italic = Regex::new(r"\*([^*\n]+?)\*").unwrap();
    html = italic.replace_all(&html, r"<em>$1</em>").to_string();

    // Convert _italic_ to <em> - safe now since __ is already converted to <strong>
    let italic2 = Regex::new(r"_([^_\n]+?)_").unwrap();
    html = italic2.replace_all(&html, r"<em>$1</em>").to_string();

    // Convert newlines to <br>
    html = html.replace("\n", "<br>\n");

    html
}

/// Strip HTML tags to create plain text version
fn strip_html_tags(html: &str) -> String {
    use regex::Regex;

    let mut text = html.to_string();

    // Convert <br> and <br/> to newlines
    let br_tag = Regex::new(r"<br\s*/?>").unwrap();
    text = br_tag.replace_all(&text, "\n").to_string();

    // Convert </p> to double newline
    text = text.replace("</p>", "\n\n");

    // Extract link text from anchors: <a href="url">text</a> -> text (url)
    let anchor = Regex::new(r#"<a[^>]+href="([^"]+)"[^>]*>([^<]+)</a>"#).unwrap();
    text = anchor.replace_all(&text, "$2 ($1)").to_string();

    // Remove all remaining HTML tags
    let tag = Regex::new(r"<[^>]+>").unwrap();
    text = tag.replace_all(&text, "").to_string();

    // Collapse multiple newlines
    let multi_newline = Regex::new(r"\n{3,}").unwrap();
    text = multi_newline.replace_all(&text, "\n\n").to_string();

    text.trim().to_string()
}

/// Encode display name for RFC 2047 (handles non-ASCII characters)
fn encode_display_name(name: &str) -> String {
    // Check if name contains only ASCII printable characters (excluding special chars)
    let needs_encoding = name.chars().any(|c| !c.is_ascii() || c == '"' || c == '\\');

    if !needs_encoding {
        // If the name contains spaces or special chars, quote it
        if name.contains(' ') || name.contains(',') || name.contains('<') || name.contains('>') {
            return format!("\"{}\"", name);
        }
        return name.to_string();
    }

    // Use RFC 2047 Base64 encoding for non-ASCII
    use base64::{engine::general_purpose::STANDARD, Engine};
    let encoded = STANDARD.encode(name.as_bytes());
    format!("=?UTF-8?B?{}?=", encoded)
}

/// Build RFC 2822 email message with optional HTML multipart
fn build_email_message(
    display_name: &str,
    from_email: &str,
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
    body: &str,
    is_html: bool,
    in_reply_to: Option<&str>,
    references: Option<&str>,
) -> String {
    let encoded_name = encode_display_name(display_name);
    let from_header = format!("{} <{}>", encoded_name, from_email);

    let mut message = format!(
        "From: {}\r\n\
         To: {}\r\n",
        from_header,
        to.join(", ")
    );

    if !cc.is_empty() {
        message.push_str(&format!("Cc: {}\r\n", cc.join(", ")));
    }
    if !bcc.is_empty() {
        message.push_str(&format!("Bcc: {}\r\n", bcc.join(", ")));
    }

    if let Some(ref msg_id) = in_reply_to {
        message.push_str(&format!("In-Reply-To: {}\r\n", msg_id));
    }
    if let Some(ref refs) = references {
        message.push_str(&format!("References: {}\r\n", refs));
    }

    message.push_str(&format!("Subject: {}\r\n", subject));
    message.push_str("MIME-Version: 1.0\r\n");

    if is_html {
        // Build multipart/alternative message
        let boundary = format!("----=_Part_{}", chrono::Utc::now().timestamp_millis());

        let html_body = convert_to_html(body);
        let plain_body = strip_html_tags(&html_body);

        message.push_str(&format!(
            "Content-Type: multipart/alternative; boundary=\"{}\"\r\n\r\n",
            boundary
        ));

        // Plain text part
        message.push_str(&format!("--{}\r\n", boundary));
        message.push_str("Content-Type: text/plain; charset=utf-8\r\n");
        message.push_str("Content-Transfer-Encoding: 7bit\r\n\r\n");
        message.push_str(&plain_body);
        message.push_str("\r\n\r\n");

        // HTML part
        message.push_str(&format!("--{}\r\n", boundary));
        message.push_str("Content-Type: text/html; charset=utf-8\r\n");
        message.push_str("Content-Transfer-Encoding: 7bit\r\n\r\n");
        message.push_str(&html_body);
        message.push_str("\r\n\r\n");

        // End boundary
        message.push_str(&format!("--{}--\r\n", boundary));
    } else {
        // Plain text only
        message.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
        message.push_str(body);
    }

    message
}

/// Parse RFC 2822 email headers and body from raw message
fn parse_email_headers(raw: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut headers = std::collections::HashMap::new();
    let mut body = String::new();
    let mut in_body = false;
    let mut current_header: Option<(String, String)> = None;

    for line in raw.lines() {
        if in_body {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
            continue;
        }

        if line.is_empty() {
            // End of headers
            if let Some((k, v)) = current_header.take() {
                headers.insert(k.to_lowercase(), v);
            }
            in_body = true;
            continue;
        }

        // Check for header continuation (starts with whitespace)
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, ref mut v)) = current_header {
                v.push(' ');
                v.push_str(line.trim());
            }
            continue;
        }

        // New header
        if let Some((k, v)) = current_header.take() {
            headers.insert(k.to_lowercase(), v);
        }

        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim().to_string();
            let value = line[colon_pos + 1..].trim().to_string();
            current_header = Some((key, value));
        }
    }

    // Handle final header
    if let Some((k, v)) = current_header {
        headers.insert(k.to_lowercase(), v);
    }

    (headers, body)
}

/// Extract email addresses from a header value like "Name <email>, Name2 <email2>"
fn parse_email_addresses(header_value: &str) -> Vec<String> {
    use regex::Regex;
    let email_pattern =
        Regex::new(r"<([^>]+)>|([a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,})").unwrap();

    email_pattern
        .captures_iter(header_value)
        .filter_map(|cap| cap.get(1).or(cap.get(2)).map(|m| m.as_str().to_string()))
        .collect()
}

/// Tool execution handler
pub struct ToolHandler {
    db: Arc<Database>,
    config: Arc<Config>,
    search: Arc<SearchEngine>,
    oauth: Arc<OAuthManager>,
}

impl ToolHandler {
    /// Create a new tool handler
    pub fn new(
        db: Arc<Database>,
        config: Arc<Config>,
        search: Arc<SearchEngine>,
        oauth: Arc<OAuthManager>,
    ) -> Self {
        Self {
            db,
            config,
            search,
            oauth,
        }
    }

    /// Execute a tool
    pub async fn execute(&self, name: &str, arguments: &Value) -> Result<Value> {
        debug!("Executing tool: {} with args: {:?}", name, arguments);

        let result = match name {
            // Management tools
            "manage_accounts" => self.manage_accounts(arguments).await,
            "manage_sync" => self.manage_sync(arguments).await,
            "manage_daemon" => self.manage_daemon(arguments).await,
            // Email tools
            "search_emails" => self.search_emails(arguments).await,
            "list_emails" => self.list_recent_emails(arguments).await,
            "get_email" => self.get_email(arguments).await,
            "get_thread" => self.get_thread(arguments).await,
            "send_email" => self.send_email(arguments).await,
            "list_folders" => self.list_folders(arguments).await,
            "get_attachment" => self.get_attachment(arguments).await,
            // Draft tools
            "create_draft" => self.create_draft(arguments).await,
            "list_drafts" => self.list_drafts(arguments).await,
            "get_draft" => self.get_draft(arguments).await,
            "update_draft" => self.update_draft(arguments).await,
            "send_draft" => self.send_draft(arguments).await,
            "delete_draft" => self.delete_draft(arguments).await,
            // Calendar tools
            "search_events" => self.search_calendar(arguments).await,
            "get_event" => self.get_event(arguments).await,
            "list_events" => self.list_calendar_events(arguments).await,
            "list_calendars" => self.list_calendars(arguments).await,
            "create_event" => self.create_event(arguments).await,
            _ => Err(Error::ToolNotFound(name.to_string())),
        }?;

        Ok(serde_json::json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&result)?
            }]
        }))
    }

    /// Manage accounts - list, get, add, delete, configure
    async fn manage_accounts(&self, args: &Value) -> Result<Value> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing action".to_string()))?;

        match action {
            "list" => self.list_accounts().await,
            "get" => self.get_account(args).await,
            "add" => self.add_account(args).await,
            "delete" => self.delete_account(args).await,
            "configure" => self.configure_account(args).await,
            _ => Err(Error::InvalidRequest(format!(
                "Unknown action '{}'. Use: list, get, add, delete, configure",
                action
            ))),
        }
    }

    /// Delete an account and its data
    async fn delete_account(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let confirm = args["confirm"].as_bool().unwrap_or(false);
        if !confirm {
            return Err(Error::InvalidRequest(
                "Must set confirm: true to delete an account".to_string(),
            ));
        }

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        // Get account info before deletion
        let account = self
            .db
            .get_account(&email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.clone()))?;

        // Delete synced data
        let (email_count, event_count) = self.db.clear_account_sync_data(&email).await?;

        // Delete account record
        self.db.delete_account(&email).await?;

        // Delete tokens
        if let Err(e) = self.oauth.token_provider().delete_tokens(&email).await {
            warn!("Failed to delete tokens for {}: {}", email, e);
        }

        Ok(serde_json::json!({
            "success": true,
            "message": format!("Account {} deleted", email),
            "deleted": {
                "account": account.id,
                "emails": email_count,
                "events": event_count
            }
        }))
    }

    /// Configure account settings
    async fn configure_account(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        // Get current account
        let mut account = self
            .db
            .get_account(&email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.clone()))?;

        // Track what changed
        let mut changes = vec![];

        // Update alias if provided
        if let Some(alias) = args.get("alias") {
            if alias.is_null() {
                if account.alias.is_some() {
                    account.alias = None;
                    changes.push("alias removed".to_string());
                }
            } else if let Some(new_alias) = alias.as_str() {
                account.alias = Some(new_alias.to_string());
                changes.push(format!("alias set to '{}'", new_alias));
            }
        }

        // Update sync_attachments if provided
        if let Some(sync_attachments) = args.get("sync_attachments").and_then(|v| v.as_bool()) {
            if account.sync_attachments != sync_attachments {
                account.sync_attachments = sync_attachments;
                changes.push(format!(
                    "sync_attachments: {} (restart daemon to apply)",
                    sync_attachments
                ));
            }
        }

        // Save account changes to DB
        if !changes.is_empty() {
            self.db.upsert_account(&account).await?;
        }

        // Handle config file settings (sync_email, sync_calendar, folders)
        // Note: These would need to be persisted to config.toml
        // For now, return what would be configured
        let mut config_changes = vec![];

        if let Some(sync_email) = args.get("sync_email").and_then(|v| v.as_bool()) {
            config_changes.push(format!("sync_email: {}", sync_email));
        }

        if let Some(sync_calendar) = args.get("sync_calendar").and_then(|v| v.as_bool()) {
            config_changes.push(format!("sync_calendar: {}", sync_calendar));
        }

        if let Some(folders) = args.get("folders").and_then(|v| v.as_array()) {
            let folder_list: Vec<String> = folders
                .iter()
                .filter_map(|f| f.as_str().map(|s| s.to_string()))
                .collect();
            if folder_list.is_empty() {
                config_changes.push("folders: all".to_string());
            } else {
                config_changes.push(format!("folders: {:?}", folder_list));
            }
        }

        changes.extend(config_changes);

        if changes.is_empty() {
            return Ok(serde_json::json!({
                "success": true,
                "message": "No changes specified",
                "account": {
                    "id": account.id,
                    "alias": account.alias,
                    "sync_attachments": account.sync_attachments
                }
            }));
        }

        Ok(serde_json::json!({
            "success": true,
            "message": format!("Account {} configured", email),
            "changes": changes,
            "account": {
                "id": account.id,
                "alias": account.alias,
                "sync_attachments": account.sync_attachments
            },
            "note": "Restart the daemon for sync_email/sync_calendar/folders/sync_attachments changes to take effect"
        }))
    }

    /// List all accounts
    async fn list_accounts(&self) -> Result<Value> {
        let accounts = self.db.list_accounts().await?;
        Ok(serde_json::json!({
            "accounts": accounts.iter().map(|a| serde_json::json!({
                "id": a.id,
                "alias": a.alias,
                "display_name": a.display_name,
                "status": format!("{:?}", a.status).to_lowercase(),
                "added_at": a.added_at.to_rfc3339(),
                "sync_attachments": a.sync_attachments
            })).collect::<Vec<_>>()
        }))
    }

    /// Get a specific account
    async fn get_account(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        let account = self
            .db
            .get_account(&email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.clone()))?;

        Ok(serde_json::to_value(&account)?)
    }

    /// Add a new Google account via OAuth
    async fn add_account(&self, args: &Value) -> Result<Value> {
        let alias = args["alias"].as_str().map(|s| s.to_string());

        // Require years_to_sync - prompt user if not provided
        let years_to_sync_str = match args.get("years_to_sync").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return Ok(serde_json::json!({
                    "success": false,
                    "needs_input": true,
                    "message": "Please specify how much email history to sync",
                    "prompt": "How many years of email history should I sync?",
                    "options": [
                        {"value": "1", "label": "1 year (recommended for most users)"},
                        {"value": "2", "label": "2 years"},
                        {"value": "5", "label": "5 years"},
                        {"value": "10", "label": "10 years"},
                        {"value": "all", "label": "All email history (may take a long time)"}
                    ],
                    "parameter": "years_to_sync",
                    "note": "You can always sync more history later with manage_sync action: 'extend'"
                }));
            }
        };

        // Parse years_to_sync: "1"-"20" for specific years, "all" for no limit
        let years_to_sync: Option<u32> = if years_to_sync_str.eq_ignore_ascii_case("all") {
            None // No limit
        } else {
            let years = years_to_sync_str.parse::<u32>().map_err(|_| {
                Error::InvalidRequest(format!(
                    "Invalid years_to_sync value '{}'. Use '1'-'20' or 'all'",
                    years_to_sync_str
                ))
            })?;
            if years < 1 || years > 20 {
                return Err(Error::InvalidRequest(
                    "years_to_sync must be between 1 and 20, or 'all'".to_string(),
                ));
            }
            Some(years)
        };

        // Generate state for CSRF protection
        let state = format!("groundeffect_{}", uuid::Uuid::new_v4());

        // Generate authorization URL
        let auth_url = self.oauth.authorization_url(&state);

        // Try to bind to the OAuth callback port
        let listener = TcpListener::bind("127.0.0.1:8085").await.map_err(|e| {
            Error::Other(format!(
                "Failed to start OAuth callback server: {}. Is another instance running?",
                e
            ))
        })?;

        // Open the browser
        if let Err(e) = open::that(&auth_url) {
            warn!("Failed to open browser automatically: {}", e);
        }

        info!("Waiting for OAuth callback on http://localhost:8085 ...");

        // Wait for callback with timeout (5 minutes)
        let callback_result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            self.wait_for_oauth_callback(&listener, &state),
        )
        .await;

        let code = match callback_result {
            Ok(Ok(code)) => code,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(Error::Other(
                    "OAuth timeout: no callback received within 5 minutes".to_string(),
                ))
            }
        };

        // Exchange code for tokens
        let (tokens, user_info) = self.oauth.exchange_code(&code).await?;

        // Store tokens
        self.oauth
            .token_provider()
            .store_tokens(&user_info.email, &tokens)
            .await?;

        // Calculate sync_email_since based on years_to_sync
        use chrono::Duration;
        let sync_since = years_to_sync.map(|years| Utc::now() - Duration::days(years as i64 * 365));

        // Check if account already exists
        if let Some(existing) = self.db.get_account(&user_info.email).await? {
            let mut updated = existing;
            updated.status = AccountStatus::Active;
            updated.alias = alias.or(updated.alias);
            updated.sync_email_since = sync_since;
            self.db.upsert_account(&updated).await?;

            Ok(serde_json::json!({
                "success": true,
                "message": format!("Account {} re-authenticated successfully", user_info.email),
                "account": {
                    "id": updated.id,
                    "alias": updated.alias,
                    "display_name": updated.display_name,
                    "status": "active",
                    "years_to_sync": years_to_sync_str
                }
            }))
        } else {
            // Create new account
            let account = Account {
                id: user_info.email.clone(),
                alias,
                display_name: user_info.name.unwrap_or_else(|| user_info.email.clone()),
                added_at: Utc::now(),
                last_sync_email: None,
                last_sync_calendar: None,
                status: AccountStatus::Active,
                sync_email_since: sync_since,
                oldest_email_synced: None,
                oldest_event_synced: None,
                sync_attachments: false, // Off by default
                estimated_total_emails: None,
            };
            self.db.upsert_account(&account).await?;

            Ok(serde_json::json!({
                "success": true,
                "message": format!("Account {} added successfully", account.id),
                "account": {
                    "id": account.id,
                    "alias": account.alias,
                    "display_name": account.display_name,
                    "status": "active",
                    "years_to_sync": years_to_sync_str
                },
                "next_steps": "Use manage_daemon with action: 'start' to begin syncing"
            }))
        }
    }

    /// Wait for OAuth callback and return the authorization code
    async fn wait_for_oauth_callback(
        &self,
        listener: &TcpListener,
        expected_state: &str,
    ) -> Result<String> {
        // Accept one connection
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|e| Error::Other(format!("Failed to accept OAuth callback: {}", e)))?;

        // Read the HTTP request
        let mut reader = BufReader::new(&mut socket);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .map_err(|e| Error::Other(format!("Failed to read OAuth callback: {}", e)))?;

        // Parse the request to extract code and state
        let (code, received_state) = self.parse_oauth_callback(&request_line)?;

        // Verify state
        if received_state != expected_state {
            let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<h1>Error: Invalid state</h1>";
            let _ = socket.write_all(response.as_bytes()).await;
            return Err(Error::Other(
                "OAuth state mismatch - possible CSRF attack".to_string(),
            ));
        }

        // Send success response to browser
        let success_html = r#"HTTP/1.1 200 OK
Content-Type: text/html; charset=utf-8

<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>GroundEffect - Success</title>
</head>
<body style="font-family: -apple-system, BlinkMacSystemFont, sans-serif; padding: 40px; text-align: center;">
    <h1>Authentication Successful!</h1>
    <p>You can close this window and return to Claude Code.</p>
</body>
</html>"#;
        let _ = socket.write_all(success_html.as_bytes()).await;

        Ok(code)
    }

    /// Parse OAuth callback URL to extract code and state
    fn parse_oauth_callback(&self, request_line: &str) -> Result<(String, String)> {
        // Request line looks like: GET /oauth/callback?code=xxx&state=yyy HTTP/1.1
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(Error::Other("Invalid HTTP request".to_string()));
        }

        let path = parts[1];
        if !path.starts_with("/oauth/callback") {
            return Err(Error::Other(format!("Unexpected callback path: {}", path)));
        }

        // Parse query string
        let query_start = path
            .find('?')
            .ok_or_else(|| Error::Other("No query string in callback".to_string()))?;
        let query = &path[query_start + 1..];

        let mut code = None;
        let mut state = None;

        for param in query.split('&') {
            let mut kv = param.splitn(2, '=');
            let key = kv.next().unwrap_or("");
            let value = kv.next().unwrap_or("");
            match key {
                "code" => {
                    code = Some(
                        urlencoding::decode(value)
                            .map_err(|e| Error::Other(format!("Failed to decode code: {}", e)))?
                            .into_owned(),
                    )
                }
                "state" => {
                    state = Some(
                        urlencoding::decode(value)
                            .map_err(|e| Error::Other(format!("Failed to decode state: {}", e)))?
                            .into_owned(),
                    )
                }
                _ => {}
            }
        }

        let code =
            code.ok_or_else(|| Error::Other("No authorization code in callback".to_string()))?;
        let state = state.ok_or_else(|| Error::Other("No state in callback".to_string()))?;

        Ok((code, state))
    }

    /// Search emails
    async fn search_emails(&self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing query".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        // Check for listing intent (fast path)
        let intent = args["intent"].as_str().unwrap_or("search");
        if intent == "list" {
            info!("Listing intent detected, using fast list path");
            return self.list_recent_emails(args).await;
        }

        // For wildcard/empty queries, use fast path (no semantic search needed)
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() || query_trimmed == "*" {
            info!("Wildcard query detected, using fast list path");
            return self.list_recent_emails(args).await;
        }

        // Resolve account aliases
        let accounts = args["accounts"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|id| self.config.resolve_account(id))
                .collect::<Vec<_>>()
        });

        // Parse date filters (format: YYYY-MM-DD) with timezone support
        let tz: Tz = self.config.general.timezone.parse().unwrap_or(Tz::UTC);
        let date_from = args["date_from"].as_str().and_then(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| {
                let naive_dt = d.and_time(NaiveTime::MIN);
                tz.from_local_datetime(&naive_dt)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
            })
        });
        let date_to = args["date_to"].as_str().and_then(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| {
                let naive_dt = d.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
                tz.from_local_datetime(&naive_dt)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
            })
        });

        let options = SearchOptions {
            accounts,
            limit,
            folder: args["folder"].as_str().map(|s| s.to_string()),
            from: args["from"].as_str().map(|s| s.to_string()),
            to: args["to"].as_str().map(|s| s.to_string()),
            date_from,
            date_to,
            has_attachment: args["has_attachment"].as_bool(),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let results = self.search.search_emails(query, &options).await?;
        let search_time = start.elapsed().as_millis();

        // If no account filter, show all accounts as searched
        let accounts_searched = match &options.accounts {
            Some(accts) => accts.clone(),
            None => self
                .db
                .list_accounts()
                .await?
                .into_iter()
                .map(|a| a.id)
                .collect(),
        };

        Ok(serde_json::json!({
            "results": results,
            "accounts_searched": accounts_searched,
            "total_count": results.len(),
            "search_time_ms": search_time
        }))
    }

    /// List recent emails (fast, no search)
    async fn list_recent_emails(&self, args: &Value) -> Result<Value> {
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;
        let limit = limit.min(100); // Cap at 100

        // Resolve account if provided (handle both "account" and "accounts" params)
        let account_id = args["account"]
            .as_str()
            .and_then(|id| self.config.resolve_account(id))
            .or_else(|| {
                // Also check "accounts" array (for search_emails redirect)
                args["accounts"]
                    .as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .and_then(|id| self.config.resolve_account(id))
            });

        info!(
            "Listing recent emails: account={:?}, limit={}",
            account_id, limit
        );

        let start = std::time::Instant::now();
        let emails = self
            .db
            .list_recent_emails(account_id.as_deref(), limit)
            .await?;
        let query_time = start.elapsed().as_millis();

        // Convert to summaries
        let results: Vec<_> = emails
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "subject": e.subject,
                    "from": e.from.to_string_full(),
                    "date": e.date.to_rfc3339(),
                    "snippet": e.snippet,
                    "folder": e.folder,
                    "is_read": e.is_read(),
                    "has_attachments": e.has_attachments(),
                    "attachments": e.attachments.iter().map(|a| serde_json::json!({
                        "id": a.id,
                        "filename": a.filename,
                        "mime_type": a.mime_type,
                        "size_human": a.size_human(),
                        "downloaded": a.downloaded
                    })).collect::<Vec<_>>()
                })
            })
            .collect();

        Ok(serde_json::json!({
            "emails": results,
            "count": results.len(),
            "query_time_ms": query_time
        }))
    }

    /// Maximum body size in chars (~40K chars ≈ 20K tokens with JSON overhead, staying under Claude Code's 25K token limit)
    const MAX_BODY_CHARS: usize = 40_000;

    /// Get a single email
    async fn get_email(&self, args: &Value) -> Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing id".to_string()))?;

        let email = self
            .db
            .get_email(id)
            .await?
            .ok_or_else(|| Error::EmailNotFound(id.to_string()))?;

        let body = email.resolved_body();

        // Check if truncation needed
        let total_chars = body.len();
        let (body_text, truncated) = if total_chars > Self::MAX_BODY_CHARS {
            // Truncate at char boundary
            let truncated_body = body
                .char_indices()
                .take_while(|(i, _)| *i < Self::MAX_BODY_CHARS)
                .map(|(_, c)| c)
                .collect::<String>();
            (truncated_body, true)
        } else {
            (body, false)
        };

        // Build response excluding body_html (embedding already skipped via #[serde(skip)])
        let mut response = serde_json::json!({
            "id": email.id,
            "account_id": email.account_id,
            "account_alias": email.account_alias,
            "message_id": email.message_id,
            "gmail_thread_id": email.gmail_thread_id,
            "folder": email.folder,
            "labels": email.labels,
            "from": email.from,
            "to": email.to,
            "cc": email.cc,
            "subject": email.subject,
            "date": email.date,
            "body": body_text,
            "snippet": email.snippet,
            "attachments": email.attachments,
            "is_read": email.is_read(),
            "is_flagged": email.is_flagged(),
        });

        if truncated {
            response["truncated"] = serde_json::json!(true);
            response["total_body_chars"] = serde_json::json!(total_chars);
        }

        Ok(response)
    }

    /// Get all emails in a thread
    async fn get_thread(&self, args: &Value) -> Result<Value> {
        let thread_id_str = args["thread_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing thread_id".to_string()))?;

        let thread_id: u64 = thread_id_str
            .parse()
            .map_err(|_| Error::InvalidRequest("thread_id must be a valid number".to_string()))?;

        // Resolve account filter if provided
        let account_id = args["accounts"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .and_then(|id| self.config.resolve_account(id));

        let emails = self
            .db
            .get_emails_by_thread(thread_id, account_id.as_deref())
            .await?;

        if emails.is_empty() {
            return Err(Error::Other(format!(
                "No emails found for thread {}",
                thread_id
            )));
        }

        // Format each email in the thread
        let mut messages = Vec::with_capacity(emails.len());
        for email in &emails {
            let body = email.resolved_body();

            // Truncate if needed
            let total_chars = body.len();
            let (body_text, truncated) = if total_chars > Self::MAX_BODY_CHARS {
                let truncated_body = body
                    .char_indices()
                    .take_while(|(i, _)| *i < Self::MAX_BODY_CHARS)
                    .map(|(_, c)| c)
                    .collect::<String>();
                (truncated_body, true)
            } else {
                (body, false)
            };

            let mut msg = serde_json::json!({
                "id": email.id,
                "from": email.from,
                "to": email.to,
                "cc": email.cc,
                "subject": email.subject,
                "date": email.date,
                "body": body_text,
                "snippet": email.snippet,
                "is_read": email.is_read(),
            });

            if truncated {
                msg["truncated"] = serde_json::json!(true);
                msg["total_body_chars"] = serde_json::json!(total_chars);
            }

            messages.push(msg);
        }

        // Use first email for thread metadata
        let first = &emails[0];
        Ok(serde_json::json!({
            "thread_id": thread_id,
            "account_id": first.account_id,
            "subject": first.subject,
            "message_count": emails.len(),
            "messages": messages,
        }))
    }

    /// Send an email via Gmail API (with optional preview mode)
    async fn send_email(&self, args: &Value) -> Result<Value> {
        // Check flags
        let confirm = args["confirm"].as_bool().unwrap_or(false);
        let save_as_draft = args["save_as_draft"].as_bool().unwrap_or(false);
        let force_html = args["html"].as_bool().unwrap_or(false);

        // Get account
        let from_account = args["from_account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing from_account".to_string()))?;

        let from_email = self
            .config
            .resolve_account(from_account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", from_account)))?;

        // Get account display name from database
        let account = self.db.get_account(&from_email).await?.ok_or_else(|| {
            Error::InvalidRequest(format!("Account not found in database: {}", from_email))
        })?;
        let display_name = &account.display_name;

        let to: Vec<String> = args["to"]
            .as_array()
            .ok_or_else(|| Error::InvalidRequest("Missing 'to' recipients".to_string()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        if to.is_empty() {
            return Err(Error::InvalidRequest(
                "At least one recipient required".to_string(),
            ));
        }

        let subject = args["subject"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing subject".to_string()))?;

        let body = args["body"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing body".to_string()))?;

        let cc: Vec<String> = args["cc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let bcc: Vec<String> = args["bcc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Build reply headers if replying
        let reply_to_id = args["reply_to_id"].as_str();
        let mut in_reply_to = None;
        let mut references = None;
        let mut final_subject = subject.to_string();

        if let Some(reply_id) = reply_to_id {
            if let Ok(Some(original)) = self.db.get_email(reply_id).await {
                in_reply_to = Some(original.message_id.clone());
                references = Some(original.message_id.clone());
                if !final_subject.starts_with("Re:") && !final_subject.starts_with("RE:") {
                    final_subject = format!("Re: {}", original.subject);
                }
            }
        }

        // Detect if HTML formatting is needed
        let is_html = force_html || detect_html_content(body);

        // If not confirmed and not saving as draft, return preview for user approval
        if !confirm && !save_as_draft {
            return Ok(serde_json::json!({
                "status": "preview",
                "message": "Please review this email. Call send_email again with confirm=true to send, or save_as_draft=true to save as draft.",
                "email": {
                    "from": format!("{} <{}>", display_name, from_email),
                    "to": to,
                    "cc": cc,
                    "bcc": bcc,
                    "subject": final_subject,
                    "body": body,
                    "is_html": is_html,
                    "in_reply_to": in_reply_to,
                    "references": references,
                }
            }));
        }

        // Build RFC 2822 message
        let message = build_email_message(
            display_name,
            &from_email,
            &to,
            &cc,
            &bcc,
            &final_subject,
            body,
            is_html,
            in_reply_to.as_deref(),
            references.as_deref(),
        );

        // Base64url encode the message
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let encoded = URL_SAFE_NO_PAD.encode(message.as_bytes());

        // Get access token
        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        // If saving as draft instead of sending
        if save_as_draft {
            let response = client
                .post("https://gmail.googleapis.com/gmail/v1/users/me/drafts")
                .bearer_auth(&access_token)
                .json(&serde_json::json!({
                    "message": { "raw": encoded }
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let error_body = response.text().await.unwrap_or_default();
                return Err(Error::Other(format!(
                    "Gmail API error {}: {}",
                    status, error_body
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let draft_id = result["id"].as_str().unwrap_or("unknown");
            let message_id = result["message"]["id"].as_str().unwrap_or("unknown");

            info!("Draft created successfully: {}", draft_id);

            return Ok(serde_json::json!({
                "status": "draft_created",
                "draft_id": draft_id,
                "message_id": message_id,
                "from": format!("{} <{}>", display_name, from_email),
                "to": to,
                "subject": final_subject,
            }));
        }

        // Send via Gmail API
        let response = client
            .post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
            .bearer_auth(&access_token)
            .json(&serde_json::json!({
                "raw": encoded
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let result: serde_json::Value = response.json().await?;
        let message_id = result["id"].as_str().unwrap_or("unknown");

        info!("Email sent successfully: {}", message_id);

        Ok(serde_json::json!({
            "status": "sent",
            "message_id": message_id,
            "from": format!("{} <{}>", display_name, from_email),
            "to": to,
            "subject": final_subject,
        }))
    }

    /// List folders
    async fn list_folders(&self, _args: &Value) -> Result<Value> {
        // Return common Gmail folders
        Ok(serde_json::json!({
            "folders": [
                "INBOX",
                "[Gmail]/All Mail",
                "[Gmail]/Drafts",
                "[Gmail]/Important",
                "[Gmail]/Sent Mail",
                "[Gmail]/Spam",
                "[Gmail]/Starred",
                "[Gmail]/Trash"
            ]
        }))
    }

    /// Get an email attachment
    async fn get_attachment(&self, args: &Value) -> Result<Value> {
        let email_id = args["email_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing email_id".to_string()))?;

        let attachment_id = args["attachment_id"].as_str();
        let filename = args["filename"].as_str();

        if attachment_id.is_none() && filename.is_none() {
            return Err(Error::InvalidRequest(
                "Must provide either attachment_id or filename".to_string(),
            ));
        }

        // Get the email
        let email = self
            .db
            .get_email(email_id)
            .await?
            .ok_or_else(|| Error::InvalidRequest(format!("Email not found: {}", email_id)))?;

        // Find the attachment
        let attachment = email
            .attachments
            .iter()
            .find(|a| {
                if let Some(id) = attachment_id {
                    a.id == id
                } else if let Some(name) = filename {
                    a.filename.eq_ignore_ascii_case(name)
                } else {
                    false
                }
            })
            .ok_or_else(|| {
                let search = attachment_id.or(filename).unwrap_or("unknown");
                Error::InvalidRequest(format!("Attachment not found: {}", search))
            })?;

        // Check if downloaded
        if !attachment.downloaded {
            return Ok(serde_json::json!({
                "error": "not_downloaded",
                "message": "Attachment not downloaded. Enable sync_attachments or use manage_sync download_attachments.",
                "attachment": {
                    "id": attachment.id,
                    "filename": attachment.filename,
                    "mime_type": attachment.mime_type,
                    "size": attachment.size,
                    "size_human": attachment.size_human()
                }
            }));
        }

        let local_path = attachment.local_path.as_ref().ok_or_else(|| {
            Error::InvalidRequest("Attachment marked as downloaded but no local_path".to_string())
        })?;

        // Check if file exists
        if !local_path.exists() {
            return Err(Error::InvalidRequest(format!(
                "Attachment file missing: {:?}",
                local_path
            )));
        }

        // Determine if we should return content or just the path
        let is_text = attachment.mime_type.starts_with("text/")
            || attachment.mime_type == "application/json"
            || attachment.mime_type == "application/xml"
            || attachment.filename.ends_with(".csv")
            || attachment.filename.ends_with(".txt")
            || attachment.filename.ends_with(".md")
            || attachment.filename.ends_with(".json")
            || attachment.filename.ends_with(".xml")
            || attachment.filename.ends_with(".yaml")
            || attachment.filename.ends_with(".yml")
            || attachment.filename.ends_with(".toml")
            || attachment.filename.ends_with(".html")
            || attachment.filename.ends_with(".htm")
            || attachment.filename.ends_with(".css")
            || attachment.filename.ends_with(".js")
            || attachment.filename.ends_with(".ts")
            || attachment.filename.ends_with(".py")
            || attachment.filename.ends_with(".rs")
            || attachment.filename.ends_with(".go")
            || attachment.filename.ends_with(".java")
            || attachment.filename.ends_with(".c")
            || attachment.filename.ends_with(".cpp")
            || attachment.filename.ends_with(".h")
            || attachment.filename.ends_with(".sh")
            || attachment.filename.ends_with(".sql");

        if is_text && attachment.size < 1_000_000 {
            // Read and return text content (up to 1MB)
            match std::fs::read_to_string(local_path) {
                Ok(content) => Ok(serde_json::json!({
                    "attachment": {
                        "id": attachment.id,
                        "filename": attachment.filename,
                        "mime_type": attachment.mime_type,
                        "size": attachment.size,
                        "size_human": attachment.size_human()
                    },
                    "content_type": "text",
                    "content": content
                })),
                Err(e) => {
                    // Fall back to returning path if read fails
                    Ok(serde_json::json!({
                        "attachment": {
                            "id": attachment.id,
                            "filename": attachment.filename,
                            "mime_type": attachment.mime_type,
                            "size": attachment.size,
                            "size_human": attachment.size_human()
                        },
                        "content_type": "binary",
                        "local_path": local_path.to_string_lossy(),
                        "read_error": e.to_string(),
                        "hint": "Use Read tool on local_path to view this file"
                    }))
                }
            }
        } else {
            // Return path for binary files
            Ok(serde_json::json!({
                "attachment": {
                    "id": attachment.id,
                    "filename": attachment.filename,
                    "mime_type": attachment.mime_type,
                    "size": attachment.size,
                    "size_human": attachment.size_human()
                },
                "content_type": "binary",
                "local_path": local_path.to_string_lossy(),
                "hint": "Use Read tool on local_path to view this file"
            }))
        }
    }

    // ========================================================================
    // Draft Operations
    // ========================================================================

    /// Create a new draft directly (no preview/confirm flow)
    async fn create_draft(&self, args: &Value) -> Result<Value> {
        let force_html = args["html"].as_bool().unwrap_or(false);

        // Get account
        let from_account = args["from_account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing from_account".to_string()))?;

        let from_email = self
            .config
            .resolve_account(from_account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", from_account)))?;

        // Get account display name from database
        let account = self.db.get_account(&from_email).await?.ok_or_else(|| {
            Error::InvalidRequest(format!("Account not found in database: {}", from_email))
        })?;
        let display_name = &account.display_name;

        let to: Vec<String> = args["to"]
            .as_array()
            .ok_or_else(|| Error::InvalidRequest("Missing 'to' recipients".to_string()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        if to.is_empty() {
            return Err(Error::InvalidRequest(
                "At least one recipient required".to_string(),
            ));
        }

        let subject = args["subject"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing subject".to_string()))?;

        let body = args["body"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing body".to_string()))?;

        let cc: Vec<String> = args["cc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let bcc: Vec<String> = args["bcc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Build reply headers if replying
        let reply_to_id = args["reply_to_id"].as_str();
        let mut in_reply_to = None;
        let mut references = None;
        let mut thread_id: Option<String> = None;
        let mut final_subject = subject.to_string();

        if let Some(reply_id) = reply_to_id {
            if let Ok(Some(original)) = self.db.get_email(reply_id).await {
                in_reply_to = Some(original.message_id.clone());
                references = Some(original.message_id.clone());
                // Gmail thread ID is stored as u64, convert to string for API
                if original.gmail_thread_id != 0 {
                    thread_id = Some(original.gmail_thread_id.to_string());
                }
                if !final_subject.starts_with("Re:") && !final_subject.starts_with("RE:") {
                    final_subject = format!("Re: {}", original.subject);
                }
            }
        }

        // Detect if HTML formatting is needed
        let is_html = force_html || detect_html_content(body);

        // Build RFC 2822 message
        let message = build_email_message(
            display_name,
            &from_email,
            &to,
            &cc,
            &bcc,
            &final_subject,
            body,
            is_html,
            in_reply_to.as_deref(),
            references.as_deref(),
        );

        // Base64url encode the message
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let encoded = URL_SAFE_NO_PAD.encode(message.as_bytes());

        // Get access token and create draft
        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        // Build request body - include threadId if replying to enable proper threading
        let request_body = if let Some(ref tid) = thread_id {
            serde_json::json!({
                "message": {
                    "raw": encoded,
                    "threadId": tid
                }
            })
        } else {
            serde_json::json!({
                "message": { "raw": encoded }
            })
        };

        let response = client
            .post("https://gmail.googleapis.com/gmail/v1/users/me/drafts")
            .bearer_auth(&access_token)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let result: serde_json::Value = response.json().await?;
        let draft_id = result["id"].as_str().unwrap_or("unknown");
        let message_id = result["message"]["id"].as_str().unwrap_or("unknown");

        info!("Draft created successfully: {}", draft_id);

        Ok(serde_json::json!({
            "status": "draft_created",
            "draft_id": draft_id,
            "message_id": message_id,
            "from": format!("{} <{}>", display_name, from_email),
            "to": to,
            "subject": final_subject,
        }))
    }

    /// List all drafts for an account
    async fn list_drafts(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let from_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", account)))?;

        let limit = args["limit"].as_u64().unwrap_or(20) as usize;

        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        let response = client
            .get(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/drafts?maxResults={}",
                limit
            ))
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let result: serde_json::Value = response.json().await?;
        let drafts_array = result["drafts"].as_array();

        let mut drafts = Vec::new();

        if let Some(draft_list) = drafts_array {
            for draft in draft_list {
                let draft_id = draft["id"].as_str().unwrap_or("unknown");
                let message_id = draft["message"]["id"].as_str().unwrap_or("unknown");

                // Get full draft content to extract subject and recipients
                let draft_response = client
                    .get(format!(
                        "https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=To&metadataHeaders=Date",
                        draft_id
                    ))
                    .bearer_auth(&access_token)
                    .send()
                    .await?;

                if draft_response.status().is_success() {
                    let draft_data: serde_json::Value = draft_response.json().await?;
                    let headers = draft_data["message"]["payload"]["headers"].as_array();

                    let mut subject = String::new();
                    let mut to = Vec::new();
                    let mut date = String::new();

                    if let Some(hdrs) = headers {
                        for h in hdrs {
                            let name = h["name"].as_str().unwrap_or("");
                            let value = h["value"].as_str().unwrap_or("");
                            match name {
                                "Subject" => subject = value.to_string(),
                                "To" => to = parse_email_addresses(value),
                                "Date" => date = value.to_string(),
                                _ => {}
                            }
                        }
                    }

                    let snippet = draft_data["message"]["snippet"].as_str().unwrap_or("");

                    drafts.push(serde_json::json!({
                        "draft_id": draft_id,
                        "message_id": message_id,
                        "subject": subject,
                        "to": to,
                        "snippet": snippet,
                        "date": date,
                    }));
                }
            }
        }

        let total = result["resultSizeEstimate"]
            .as_u64()
            .unwrap_or(drafts.len() as u64);

        Ok(serde_json::json!({
            "drafts": drafts,
            "total": total,
        }))
    }

    /// Get full content of a specific draft
    async fn get_draft(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let draft_id = args["draft_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing draft_id".to_string()))?;

        let from_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", account)))?;

        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        let response = client
            .get(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=full",
                draft_id
            ))
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let draft_data: serde_json::Value = response.json().await?;
        let message_id = draft_data["message"]["id"].as_str().unwrap_or("unknown");

        // Extract headers
        let headers = draft_data["message"]["payload"]["headers"].as_array();
        let mut subject = String::new();
        let mut to = Vec::new();
        let mut cc = Vec::new();
        let mut from = String::new();
        let mut date = String::new();

        if let Some(hdrs) = headers {
            for h in hdrs {
                let name = h["name"].as_str().unwrap_or("");
                let value = h["value"].as_str().unwrap_or("");
                match name {
                    "Subject" => subject = value.to_string(),
                    "To" => to = parse_email_addresses(value),
                    "Cc" => cc = parse_email_addresses(value),
                    "From" => from = value.to_string(),
                    "Date" => date = value.to_string(),
                    _ => {}
                }
            }
        }

        // Recursively extract body from potentially nested multipart structures
        fn extract_body_recursive(
            part: &serde_json::Value,
            body: &mut String,
            body_html: &mut String,
        ) {
            use base64::{
                engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
                Engine,
            };
            let mime_type = part["mimeType"].as_str().unwrap_or("");

            if mime_type == "text/plain" && body.is_empty() {
                if let Some(body_data) = part["body"]["data"].as_str() {
                    // Gmail uses URL-safe base64 - try with padding first, then without
                    let decoded = URL_SAFE
                        .decode(body_data)
                        .or_else(|_| URL_SAFE_NO_PAD.decode(body_data));
                    if let Ok(decoded) = decoded {
                        if let Ok(text) = String::from_utf8(decoded) {
                            *body = text;
                        }
                    }
                }
            } else if mime_type == "text/html" && body_html.is_empty() {
                if let Some(body_data) = part["body"]["data"].as_str() {
                    let decoded = URL_SAFE
                        .decode(body_data)
                        .or_else(|_| URL_SAFE_NO_PAD.decode(body_data));
                    if let Ok(decoded) = decoded {
                        if let Ok(text) = String::from_utf8(decoded) {
                            *body_html = text;
                        }
                    }
                }
            } else if mime_type.starts_with("multipart/") {
                if let Some(parts) = part["parts"].as_array() {
                    for nested_part in parts {
                        extract_body_recursive(nested_part, body, body_html);
                    }
                }
            }
        }

        let mut body = String::new();
        let mut body_html = String::new();
        let payload = &draft_data["message"]["payload"];
        extract_body_recursive(payload, &mut body, &mut body_html);

        body = Email::body_for_indexing_and_display(&body, Some(&body_html));

        Ok(serde_json::json!({
            "draft_id": draft_id,
            "message_id": message_id,
            "from": from,
            "to": to,
            "cc": cc,
            "subject": subject,
            "body": body,
            "body_html": body_html,
            "date": date,
        }))
    }

    /// Update an existing draft
    async fn update_draft(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let draft_id = args["draft_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing draft_id".to_string()))?;

        let from_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", account)))?;

        // Get account display name from database
        let db_account = self.db.get_account(&from_email).await?.ok_or_else(|| {
            Error::InvalidRequest(format!("Account not found in database: {}", from_email))
        })?;
        let display_name = &db_account.display_name;

        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        // First, get the existing draft to preserve fields not being updated
        let existing_response = client
            .get(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=full",
                draft_id
            ))
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !existing_response.status().is_success() {
            let status = existing_response.status();
            let error_body = existing_response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let existing: serde_json::Value = existing_response.json().await?;

        // Extract existing values
        let headers = existing["message"]["payload"]["headers"].as_array();
        let mut existing_subject = String::new();
        let mut existing_to = Vec::new();
        let mut existing_cc = Vec::new();
        let mut existing_body = String::new();

        if let Some(hdrs) = headers {
            for h in hdrs {
                let name = h["name"].as_str().unwrap_or("");
                let value = h["value"].as_str().unwrap_or("");
                match name {
                    "Subject" => existing_subject = value.to_string(),
                    "To" => existing_to = parse_email_addresses(value),
                    "Cc" => existing_cc = parse_email_addresses(value),
                    _ => {}
                }
            }
        }

        // Recursively extract text/plain body from potentially nested multipart structures
        fn extract_text_body_recursive(part: &serde_json::Value) -> Option<String> {
            use base64::{
                engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
                Engine,
            };
            let mime_type = part["mimeType"].as_str().unwrap_or("");

            if mime_type == "text/plain" {
                if let Some(body_data) = part["body"]["data"].as_str() {
                    // Gmail uses URL-safe base64 - try with padding first, then without
                    let decoded = URL_SAFE
                        .decode(body_data)
                        .or_else(|_| URL_SAFE_NO_PAD.decode(body_data));
                    if let Ok(decoded) = decoded {
                        if let Ok(text) = String::from_utf8(decoded) {
                            return Some(text);
                        }
                    }
                }
            } else if mime_type.starts_with("multipart/") {
                if let Some(parts) = part["parts"].as_array() {
                    for nested_part in parts {
                        if let Some(text) = extract_text_body_recursive(nested_part) {
                            return Some(text);
                        }
                    }
                }
            }
            None
        }

        // Extract existing body
        let payload = &existing["message"]["payload"];
        if let Some(text) = extract_text_body_recursive(payload) {
            existing_body = text;
        }

        // Use provided values or fall back to existing
        let to: Vec<String> = args["to"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or(existing_to);

        let subject = args["subject"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or(existing_subject);

        let body = args["body"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or(existing_body);

        let cc: Vec<String> = args["cc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or(existing_cc);

        let bcc: Vec<String> = args["bcc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Detect if HTML formatting is needed
        let force_html = args["html"].as_bool().unwrap_or(false);
        let is_html = force_html || detect_html_content(&body);

        // Build RFC 2822 message
        let message = build_email_message(
            display_name,
            &from_email,
            &to,
            &cc,
            &bcc,
            &subject,
            &body,
            is_html,
            None,
            None,
        );

        // Base64url encode the message
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let encoded = URL_SAFE_NO_PAD.encode(message.as_bytes());

        // Update draft
        let response = client
            .put(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}",
                draft_id
            ))
            .bearer_auth(&access_token)
            .json(&serde_json::json!({
                "message": { "raw": encoded }
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let result: serde_json::Value = response.json().await?;
        let new_draft_id = result["id"].as_str().unwrap_or(draft_id);

        info!("Draft updated successfully: {}", new_draft_id);

        Ok(serde_json::json!({
            "status": "updated",
            "draft_id": new_draft_id,
            "from": format!("{} <{}>", display_name, from_email),
            "to": to,
            "subject": subject,
        }))
    }

    /// Send an existing draft by ID
    async fn send_draft(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let draft_id = args["draft_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing draft_id".to_string()))?;

        let from_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", account)))?;

        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        // First get draft info to return details
        let draft_response = client
            .get(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=To&metadataHeaders=From",
                draft_id
            ))
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !draft_response.status().is_success() {
            let status = draft_response.status();
            let error_body = draft_response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let draft_data: serde_json::Value = draft_response.json().await?;
        let headers = draft_data["message"]["payload"]["headers"].as_array();

        let mut subject = String::new();
        let mut to = Vec::new();
        let mut from = String::new();

        if let Some(hdrs) = headers {
            for h in hdrs {
                let name = h["name"].as_str().unwrap_or("");
                let value = h["value"].as_str().unwrap_or("");
                match name {
                    "Subject" => subject = value.to_string(),
                    "To" => to = parse_email_addresses(value),
                    "From" => from = value.to_string(),
                    _ => {}
                }
            }
        }

        // Send the draft
        let response = client
            .post("https://gmail.googleapis.com/gmail/v1/users/me/drafts/send")
            .bearer_auth(&access_token)
            .json(&serde_json::json!({
                "id": draft_id
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        let result: serde_json::Value = response.json().await?;
        let message_id = result["id"].as_str().unwrap_or("unknown");

        info!("Draft sent successfully: {} -> {}", draft_id, message_id);

        Ok(serde_json::json!({
            "status": "sent",
            "message_id": message_id,
            "draft_id": draft_id,
            "from": from,
            "to": to,
            "subject": subject,
        }))
    }

    /// Delete an existing draft by ID
    async fn delete_draft(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let draft_id = args["draft_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing draft_id".to_string()))?;

        let from_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::InvalidRequest(format!("Unknown account: {}", account)))?;

        let access_token = self.oauth.get_valid_token(&from_email).await?;
        let client = reqwest::Client::new();

        let response = client
            .delete(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}",
                draft_id
            ))
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Gmail API error {}: {}",
                status, error_body
            )));
        }

        info!("Draft deleted successfully: {}", draft_id);

        Ok(serde_json::json!({
            "status": "deleted",
            "draft_id": draft_id,
        }))
    }

    /// Search calendar events
    async fn search_calendar(&self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing query".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        // Resolve account aliases
        let accounts = args["accounts"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|id| self.config.resolve_account(id))
                .collect::<Vec<_>>()
        });

        // Parse date filters (format: YYYY-MM-DD) with timezone support
        let tz: Tz = self.config.general.timezone.parse().unwrap_or(Tz::UTC);
        let date_from = args["date_from"].as_str().and_then(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| {
                let naive_dt = d.and_time(NaiveTime::MIN);
                tz.from_local_datetime(&naive_dt)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
            })
        });
        let date_to = args["date_to"].as_str().and_then(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| {
                let naive_dt = d.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
                tz.from_local_datetime(&naive_dt)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
            })
        });

        let options = CalendarSearchOptions {
            accounts,
            limit,
            calendar_id: args["calendar_id"].as_str().map(|s| s.to_string()),
            date_from,
            date_to,
        };

        let start = std::time::Instant::now();
        let results = self.search.search_calendar(query, &options).await?;
        let search_time = start.elapsed().as_millis();

        // If no account filter, show all accounts as searched
        let accounts_searched = match &options.accounts {
            Some(accts) => accts.clone(),
            None => self
                .db
                .list_accounts()
                .await?
                .into_iter()
                .map(|a| a.id)
                .collect(),
        };

        Ok(serde_json::json!({
            "results": results,
            "accounts_searched": accounts_searched,
            "total_count": results.len(),
            "search_time_ms": search_time
        }))
    }

    /// Get a single event
    async fn get_event(&self, args: &Value) -> Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing id".to_string()))?;

        let event = self
            .db
            .get_event(id)
            .await?
            .ok_or_else(|| Error::Other(format!("Event not found: {}", id)))?;

        Ok(serde_json::to_value(&event)?)
    }

    /// List calendar events in a date range (no semantic search required)
    async fn list_calendar_events(&self, args: &Value) -> Result<Value> {
        // Get date range using user's timezone for "today"
        let tz: Tz = self.config.general.timezone.parse().unwrap_or(Tz::UTC);
        let today = chrono::Utc::now()
            .with_timezone(&tz)
            .format("%Y-%m-%d")
            .to_string();
        let from = args["from"].as_str().unwrap_or(&today).to_string();

        let to = match args["to"].as_str() {
            Some(d) => d.to_string(),
            None => {
                let from_date = chrono::NaiveDate::parse_from_str(&from, "%Y-%m-%d")
                    .unwrap_or_else(|_| chrono::Utc::now().with_timezone(&tz).date_naive());
                (from_date + chrono::Duration::days(7))
                    .format("%Y-%m-%d")
                    .to_string()
            }
        };

        let limit = args["limit"].as_u64().unwrap_or(50) as usize;

        // Resolve account filter if provided
        let accounts: Option<Vec<String>> = args["accounts"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|id| self.config.resolve_account(id))
                .collect()
        });

        let events = self
            .db
            .list_events_in_range(accounts.as_deref(), &from, &to, limit.min(200))
            .await?;

        // Convert to JSON-friendly format with full attendee/organizer data
        let results: Vec<serde_json::Value> = events.iter().map(|e| {
            serde_json::json!({
                "id": e.id,
                "summary": e.summary,
                "start": match &e.start {
                    crate::models::EventTime::DateTime(dt) => dt.to_rfc3339(),
                    crate::models::EventTime::Date(d) => d.to_string(),
                },
                "end": match &e.end {
                    crate::models::EventTime::DateTime(dt) => dt.to_rfc3339(),
                    crate::models::EventTime::Date(d) => d.to_string(),
                },
                "location": e.location,
                "organizer": e.organizer.as_ref().map(|o| serde_json::json!({
                    "email": o.email,
                    "name": o.name,
                    "response_status": o.response_status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                    "optional": o.optional,
                })),
                "attendees": e.attendees.iter().map(|a| serde_json::json!({
                    "email": a.email,
                    "name": a.name,
                    "response_status": a.response_status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                    "optional": a.optional,
                })).collect::<Vec<_>>(),
                "account_id": e.account_id,
                "calendar_id": e.calendar_id,
            })
        }).collect();

        Ok(serde_json::json!({
            "from": from,
            "to": to,
            "count": results.len(),
            "events": results
        }))
    }

    /// List calendars for all accounts (or filtered accounts)
    async fn list_calendars(&self, args: &Value) -> Result<Value> {
        // Resolve account filter if provided
        let account_filter: Option<Vec<String>> = args["accounts"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|id| self.config.resolve_account(id))
                .collect()
        });

        let accounts = self.db.list_accounts().await?;
        let mut all_calendars = Vec::new();

        for account in &accounts {
            // Skip if account filter is provided and this account isn't in it
            if let Some(ref filter) = account_filter {
                if !filter.contains(&account.id) {
                    continue;
                }
            }

            // Get access token for this account
            let access_token = match self.oauth.get_valid_token(&account.id).await {
                Ok(token) => token,
                Err(e) => {
                    warn!("Failed to get token for {}: {}", account.id, e);
                    continue;
                }
            };

            // Call Google Calendar API to list calendars
            let client = reqwest::Client::new();
            let response = client
                .get("https://www.googleapis.com/calendar/v3/users/me/calendarList")
                .bearer_auth(&access_token)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(items) = json["items"].as_array() {
                            for item in items {
                                all_calendars.push(serde_json::json!({
                                    "account_id": account.id,
                                    "account_alias": account.alias,
                                    "id": item["id"],
                                    "summary": item["summary"],
                                    "description": item["description"],
                                    "primary": item["primary"].as_bool().unwrap_or(false),
                                    "access_role": item["accessRole"],
                                    "background_color": item["backgroundColor"],
                                    "foreground_color": item["foregroundColor"],
                                    "selected": item["selected"].as_bool().unwrap_or(true),
                                    "time_zone": item["timeZone"]
                                }));
                            }
                        }
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    warn!(
                        "Failed to list calendars for {}: {} - {}",
                        account.id, status, body
                    );
                }
                Err(e) => {
                    warn!("Failed to list calendars for {}: {}", account.id, e);
                }
            }
        }

        Ok(serde_json::json!({
            "calendars": all_calendars,
            "count": all_calendars.len()
        }))
    }

    /// Create a calendar event via Google Calendar API
    async fn create_event(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let account_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::AccountNotFound(account.to_string()))?;

        let summary = args["summary"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing summary (event title)".to_string()))?;

        let start = args["start"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing start time (ISO 8601)".to_string()))?;

        let end = args["end"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing end time (ISO 8601)".to_string()))?;

        // Optional fields
        let calendar_id = args["calendar_id"].as_str().unwrap_or("primary");
        let description = args["description"].as_str();
        let location = args["location"].as_str();
        let attendees: Vec<String> = args["attendees"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Build the event object for Google Calendar API
        let mut event_body = serde_json::json!({
            "summary": summary,
            "start": {
                "dateTime": start,
                "timeZone": "UTC"
            },
            "end": {
                "dateTime": end,
                "timeZone": "UTC"
            }
        });

        if let Some(desc) = description {
            event_body["description"] = serde_json::json!(desc);
        }

        if let Some(loc) = location {
            event_body["location"] = serde_json::json!(loc);
        }

        if !attendees.is_empty() {
            event_body["attendees"] = serde_json::json!(attendees
                .iter()
                .map(|email| serde_json::json!({"email": email}))
                .collect::<Vec<_>>());
        }

        // Get access token
        let access_token = self.oauth.get_valid_token(&account_email).await?;

        // Create event via Google Calendar API
        let client = reqwest::Client::new();
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            urlencoding::encode(calendar_id)
        );

        let response = client
            .post(&url)
            .bearer_auth(&access_token)
            .json(&event_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "Failed to create event: {} - {}",
                status, error_body
            )));
        }

        let created_event: serde_json::Value = response.json().await?;
        let event_id = created_event["id"].as_str().unwrap_or("unknown");
        let html_link = created_event["htmlLink"].as_str();

        info!("Created calendar event: {} for {}", event_id, account_email);

        Ok(serde_json::json!({
            "success": true,
            "message": format!("Event '{}' created successfully", summary),
            "event": {
                "id": event_id,
                "summary": summary,
                "start": start,
                "end": end,
                "calendar_id": calendar_id,
                "account": account_email,
                "html_link": html_link
            }
        }))
    }

    /// Get sync status for all accounts
    async fn sync_status_all(&self) -> Result<Value> {
        // Refresh table handles to see latest data from daemon
        self.db.refresh_tables().await?;

        let accounts = self.db.list_accounts().await?;

        // Try to read daemon's progress file for live sync progress
        let progress_file = self.config.sync_progress_file();
        let sync_progress: Option<Vec<crate::sync::AccountSyncState>> =
            std::fs::read_to_string(&progress_file)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());

        let mut account_stats = Vec::new();
        let mut total_emails = 0u64;
        let mut total_events = 0u64;
        let mut total_attachments = 0usize;
        let mut total_downloaded = 0usize;
        let mut total_attachment_size = 0u64;

        for account in &accounts {
            let email_count = self.db.count_emails(Some(&account.id)).await?;
            let event_count = self.db.count_events(Some(&account.id)).await?;
            let (att_total, att_downloaded, att_size) = self
                .db
                .get_attachment_stats(&account.id)
                .await
                .unwrap_or((0, 0, 0));

            total_emails += email_count;
            total_events += event_count;
            total_attachments += att_total;
            total_downloaded += att_downloaded;
            total_attachment_size += att_size;

            // Check for live progress from daemon
            let live_progress = sync_progress
                .as_ref()
                .and_then(|states| states.iter().find(|s| s.account_id == account.id));

            // Build sync progress if available
            let (is_syncing, sync_progress_json) = if let Some(progress_state) = live_progress {
                let progress_json = progress_state
                    .initial_sync_progress
                    .as_ref()
                    .map(|progress| {
                        serde_json::json!({
                            "phase": format!("{:?}", progress.phase),
                            "emails_synced": progress.emails_synced,
                            "total_emails_estimated": progress.total_emails_estimated,
                            "events_synced": progress.events_synced,
                            "total_events_estimated": progress.total_events_estimated,
                            "percentage_complete": progress.percentage_complete(),
                            "emails_per_second": progress.emails_per_second,
                            "estimated_seconds_remaining": progress.estimated_seconds_remaining()
                        })
                    });
                (progress_state.is_syncing, progress_json)
            } else {
                (false, None)
            };

            // Helper to format UTC time as local time string
            fn format_local_time(dt: DateTime<Utc>) -> String {
                let local: DateTime<Local> = dt.into();
                local.format("%Y-%m-%d %H:%M:%S %Z").to_string()
            }

            // Always include all fields for consistent output
            let stat = serde_json::json!({
                "id": account.id,
                "alias": account.alias,
                "status": format!("{:?}", account.status).to_lowercase(),
                "sync_target_date": account.sync_email_since.map(|d| d.format("%Y-%m-%d").to_string()),
                "oldest_email_synced": account.oldest_email_synced.map(|d| d.format("%Y-%m-%d").to_string()),
                "oldest_event_synced": account.oldest_event_synced.map(|d| d.format("%Y-%m-%d").to_string()),
                "last_email_sync": account.last_sync_email.map(format_local_time),
                "last_calendar_sync": account.last_sync_calendar.map(format_local_time),
                "email_count": email_count,
                "event_count": event_count,
                "sync_attachments_enabled": account.sync_attachments,
                "attachments": {
                    "total": att_total,
                    "downloaded": att_downloaded,
                    "pending": att_total - att_downloaded
                },
                "is_syncing": is_syncing,
                "sync_progress": sync_progress_json
            });

            account_stats.push(stat);
        }

        Ok(serde_json::json!({
            "accounts": account_stats,
            "totals": {
                "email_count": total_emails,
                "event_count": total_events,
                "attachments": {
                    "total": total_attachments,
                    "downloaded": total_downloaded,
                    "pending": total_attachments - total_downloaded,
                    "storage_bytes": total_attachment_size,
                    "storage_human": format_bytes(total_attachment_size)
                }
            }
        }))
    }

    /// Manage sync - status (for one or all accounts), reset, extend, resume_from
    async fn manage_sync(&self, args: &Value) -> Result<Value> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing action".to_string()))?;

        // Account is optional for "status" action
        let account_id = args["account"].as_str();

        match action {
            "status" => {
                // If no account specified, show status for all accounts
                if let Some(id) = account_id {
                    let email = self
                        .config
                        .resolve_account(id)
                        .ok_or_else(|| Error::AccountNotFound(id.to_string()))?;
                    self.sync_status(&email).await
                } else {
                    self.sync_status_all().await
                }
            }
            "reset" | "extend" | "resume_from" | "download_attachments" => {
                // These actions require an account
                let id = account_id
                    .ok_or_else(|| Error::InvalidRequest(format!("Action '{}' requires an account", action)))?;
                let email = self
                    .config
                    .resolve_account(id)
                    .ok_or_else(|| Error::AccountNotFound(id.to_string()))?;

                match action {
                    "reset" => self.sync_reset(&email, args).await,
                    "extend" => self.sync_extend(&email, args).await,
                    "resume_from" => self.sync_resume_from(&email, args).await,
                    "download_attachments" => self.sync_download_attachments(&email).await,
                    _ => unreachable!(),
                }
            }
            _ => Err(Error::InvalidRequest(format!(
                "Unknown action '{}'. Use: status, reset, extend, resume_from, download_attachments",
                action
            ))),
        }
    }

    /// Get sync status for an account
    async fn sync_status(&self, email: &str) -> Result<Value> {
        use chrono::Duration;

        let account = self
            .db
            .get_account(email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.to_string()))?;

        let current_sync_from = account
            .sync_email_since
            .unwrap_or_else(|| Utc::now() - Duration::days(90));
        let oldest_email_synced = account.oldest_email_synced;
        let oldest_event_synced = account.oldest_event_synced;
        let email_count = self.db.count_emails(Some(email)).await?;
        let event_count = self.db.count_events(Some(email)).await?;

        // Get attachment stats
        let (total_attachments, downloaded_attachments, attachment_size) = self
            .db
            .get_attachment_stats(email)
            .await
            .unwrap_or((0, 0, 0));

        Ok(serde_json::json!({
            "account": email,
            "sync_status": {
                "configured_sync_from": current_sync_from.format("%Y-%m-%d").to_string(),
                "oldest_email_synced": oldest_email_synced.map(|d| d.format("%Y-%m-%d").to_string()),
                "oldest_event_synced": oldest_event_synced.map(|d| d.format("%Y-%m-%d").to_string()),
                "last_sync_email": account.last_sync_email.map(|d| d.format("%Y-%m-%d %H:%M").to_string()),
                "last_sync_calendar": account.last_sync_calendar.map(|d| d.format("%Y-%m-%d %H:%M").to_string()),
                "email_count": email_count,
                "event_count": event_count,
                "sync_attachments_enabled": account.sync_attachments,
                "attachments": {
                    "total": total_attachments,
                    "downloaded": downloaded_attachments,
                    "pending": total_attachments - downloaded_attachments,
                    "total_size_bytes": attachment_size,
                    "total_size_human": format_bytes(attachment_size)
                }
            },
            "message": format!(
                "{} emails{}, {} calendar events{}, {} attachments ({} downloaded)",
                email_count,
                oldest_email_synced.map(|d| format!(" (back to {})", d.format("%Y-%m-%d"))).unwrap_or_default(),
                event_count,
                oldest_event_synced.map(|d| format!(" (back to {})", d.format("%Y-%m-%d"))).unwrap_or_default(),
                total_attachments,
                downloaded_attachments
            )
        }))
    }

    /// Reset sync data for an account
    async fn sync_reset(&self, email: &str, args: &Value) -> Result<Value> {
        let data_type = args["data_type"].as_str().unwrap_or("all");
        if !["email", "calendar", "all"].contains(&data_type) {
            return Err(Error::InvalidRequest(
                "data_type must be 'email', 'calendar', or 'all'".to_string(),
            ));
        }

        let confirm = args["confirm"].as_bool().unwrap_or(false);
        if !confirm {
            return Err(Error::InvalidRequest(
                "Must set confirm: true to reset sync data".to_string(),
            ));
        }

        // Clear sync data based on type
        let (email_count, event_count) = match data_type {
            "email" => {
                let count = self.db.clear_account_emails(email).await?;
                (count, 0)
            }
            "calendar" => {
                let count = self.db.clear_account_events(email).await?;
                (0, count)
            }
            _ => self.db.clear_account_sync_data(email).await?,
        };

        // Reset account sync timestamps based on type
        if let Some(mut account) = self.db.get_account(email).await? {
            match data_type {
                "email" => {
                    account.last_sync_email = None;
                    account.oldest_email_synced = None;
                }
                "calendar" => {
                    account.last_sync_calendar = None;
                    account.oldest_event_synced = None;
                }
                _ => {
                    account.last_sync_email = None;
                    account.last_sync_calendar = None;
                    account.oldest_email_synced = None;
                    account.oldest_event_synced = None;
                }
            }
            self.db.upsert_account(&account).await?;
        }

        Ok(serde_json::json!({
            "success": true,
            "message": format!("Reset {} sync data for {}", data_type, email),
            "deleted": {
                "emails": email_count,
                "events": event_count
            },
            "next_steps": "Use manage_daemon with action: 'start' to re-sync"
        }))
    }

    /// Extend sync range to include older data
    async fn sync_extend(&self, email: &str, args: &Value) -> Result<Value> {
        use chrono::Duration;

        let target_date = args["target_date"].as_str().ok_or_else(|| {
            Error::InvalidRequest("Missing target_date. Use YYYY-MM-DD format.".to_string())
        })?;

        let account = self
            .db
            .get_account(email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.to_string()))?;

        let current_sync_from = account
            .sync_email_since
            .unwrap_or_else(|| Utc::now() - Duration::days(90));

        // Parse target date
        let parsed_date =
            chrono::NaiveDate::parse_from_str(target_date, "%Y-%m-%d").map_err(|e| {
                Error::InvalidRequest(format!("Invalid date format: {}. Use YYYY-MM-DD", e))
            })?;

        let target_datetime = parsed_date
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| dt.and_local_timezone(chrono::Utc).single())
            .ok_or_else(|| Error::InvalidRequest("Failed to parse date".to_string()))?;

        // Validate the target date
        if target_datetime >= current_sync_from {
            return Err(Error::InvalidRequest(format!(
                "Target date {} is already within current sync range (back to {}). Choose an earlier date.",
                target_date,
                current_sync_from.format("%Y-%m-%d")
            )));
        }

        // Update the account with new sync range
        let mut updated_account = account.clone();
        updated_account.sync_email_since = Some(target_datetime);
        self.db.upsert_account(&updated_account).await?;

        Ok(serde_json::json!({
            "success": true,
            "account": email,
            "sync_range": {
                "previous_sync_from": current_sync_from.format("%Y-%m-%d").to_string(),
                "new_sync_from": target_date,
                "additional_days": (current_sync_from - target_datetime).num_days()
            },
            "message": format!(
                "Extended sync range from {} to {}. Will sync {} additional days.",
                current_sync_from.format("%Y-%m-%d"),
                target_date,
                (current_sync_from - target_datetime).num_days()
            ),
            "next_steps": "Use manage_daemon with action: 'start' to begin syncing older data"
        }))
    }

    /// Force sync to resume from a specific date
    async fn sync_resume_from(&self, email: &str, args: &Value) -> Result<Value> {
        let target_date = args["target_date"].as_str().ok_or_else(|| {
            Error::InvalidRequest("Missing target_date. Use YYYY-MM-DD format.".to_string())
        })?;

        // Parse target date
        let parsed_date =
            chrono::NaiveDate::parse_from_str(target_date, "%Y-%m-%d").map_err(|e| {
                Error::InvalidRequest(format!("Invalid date format: {}. Use YYYY-MM-DD", e))
            })?;

        let target_datetime = parsed_date
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| dt.and_local_timezone(chrono::Utc).single())
            .ok_or_else(|| Error::InvalidRequest("Failed to parse date".to_string()))?;

        let account = self
            .db
            .get_account(email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.to_string()))?;

        let old_oldest_email = account.oldest_email_synced;
        let old_oldest_event = account.oldest_event_synced;

        // Update account to force resume from the specified date
        let mut updated_account = account.clone();
        // Set oldest_synced to target_date so sync will resume from there
        updated_account.oldest_email_synced = Some(target_datetime);
        updated_account.oldest_event_synced = Some(target_datetime);
        // Clear last_sync timestamps to force full incremental check
        updated_account.last_sync_email = None;
        updated_account.last_sync_calendar = None;
        self.db.upsert_account(&updated_account).await?;

        Ok(serde_json::json!({
            "success": true,
            "account": email,
            "resume_from": target_date,
            "previous_state": {
                "oldest_email_synced": old_oldest_email.map(|d| d.format("%Y-%m-%d").to_string()),
                "oldest_event_synced": old_oldest_event.map(|d| d.format("%Y-%m-%d").to_string())
            },
            "message": format!(
                "Sync will resume from {}. Existing data is preserved; duplicates are prevented by ID matching.",
                target_date
            ),
            "next_steps": "Use manage_daemon with action: 'restart' to apply changes"
        }))
    }

    /// Download attachments for emails that have them but haven't been downloaded yet
    /// For large batches, enables sync_attachments and lets the daemon handle it in background
    async fn sync_download_attachments(&self, email: &str) -> Result<Value> {
        info!("Checking pending attachments for {}", email);

        // Get attachment stats
        let (total_attachments, downloaded_attachments, _) = self
            .db
            .get_attachment_stats(email)
            .await
            .unwrap_or((0, 0, 0));
        let pending_count = total_attachments - downloaded_attachments;

        if pending_count == 0 {
            return Ok(serde_json::json!({
                "success": true,
                "account": email,
                "pending_count": 0,
                "message": "No pending attachments to download"
            }));
        }

        // Get account to check/update sync_attachments setting
        let account = self
            .db
            .get_account(email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.to_string()))?;

        // For any pending attachments, enable sync_attachments so daemon handles it
        if !account.sync_attachments {
            let mut updated_account = account.clone();
            updated_account.sync_attachments = true;
            self.db.upsert_account(&updated_account).await?;

            Ok(serde_json::json!({
                "success": true,
                "account": email,
                "pending_count": pending_count,
                "sync_attachments_enabled": true,
                "message": format!(
                    "Found {} pending attachments. Enabled sync_attachments - daemon will download in background. Restart daemon if not running.",
                    pending_count
                )
            }))
        } else {
            // Already enabled, daemon will handle it
            Ok(serde_json::json!({
                "success": true,
                "account": email,
                "pending_count": pending_count,
                "sync_attachments_enabled": true,
                "message": format!(
                    "Found {} pending attachments. sync_attachments already enabled - daemon will download in background.",
                    pending_count
                )
            }))
        }
    }

    /// Get the path to the daemon binary (sibling of current executable)
    fn get_daemon_binary_path(&self) -> Result<std::path::PathBuf> {
        let current_exe = std::env::current_exe()
            .map_err(|e| Error::Other(format!("Failed to get current executable path: {}", e)))?;

        let exe_dir = current_exe
            .parent()
            .ok_or_else(|| Error::Other("Failed to get executable directory".to_string()))?;

        let daemon_path = exe_dir.join("groundeffect-daemon");

        if daemon_path.exists() {
            Ok(daemon_path)
        } else {
            Err(Error::Other(format!(
                "Daemon binary not found at {:?}. Make sure groundeffect-daemon is built.",
                daemon_path
            )))
        }
    }

    /// Check if daemon is running by reading PID file or using pgrep
    fn is_daemon_running(&self) -> Option<u32> {
        // First, try PID file (for daemons started via MCP tool)
        let pid_file = self.config.daemon_pid_file();

        if pid_file.exists() {
            if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    // Verify the process is actually running
                    #[cfg(unix)]
                    {
                        use std::process::Command;
                        let output = Command::new("kill")
                            .args(["-0", &pid.to_string()])
                            .output()
                            .ok();

                        if let Some(out) = output {
                            if out.status.success() {
                                return Some(pid);
                            }
                        }
                        // Process not running, clean up stale PID file
                        let _ = std::fs::remove_file(&pid_file);
                    }

                    #[cfg(not(unix))]
                    {
                        return Some(pid);
                    }
                }
            }
        }

        // Fallback: use pgrep to find daemon (for manually started daemons)
        #[cfg(unix)]
        {
            use std::process::Command;
            let output = Command::new("pgrep")
                .args(["-f", "groundeffect-daemon"])
                .output()
                .ok()?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // pgrep returns one PID per line, take the first one
                if let Some(first_line) = stdout.lines().next() {
                    if let Ok(pid) = first_line.trim().parse::<u32>() {
                        return Some(pid);
                    }
                }
            }
        }

        None
    }

    /// Manage the daemon (start, stop, restart, status)
    async fn manage_daemon(&self, arguments: &Value) -> Result<Value> {
        let action = arguments["action"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing action".to_string()))?;

        match action {
            "start" => self.daemon_start(arguments).await,
            "stop" => self.daemon_stop().await,
            "restart" => self.daemon_restart(arguments).await,
            "status" => self.daemon_status().await,
            _ => Err(Error::InvalidRequest(
                "action must be 'start', 'stop', 'restart', or 'status'".to_string(),
            )),
        }
    }

    /// Start the daemon
    async fn daemon_start(&self, arguments: &Value) -> Result<Value> {
        // Load existing daemon config
        let mut daemon_config = DaemonConfig::load().unwrap_or_default();
        let mut config_changed = false;

        // Apply parameter overrides from arguments
        if let Some(logging) = arguments.get("logging").and_then(|v| v.as_bool()) {
            if daemon_config.logging_enabled != logging {
                daemon_config.logging_enabled = logging;
                config_changed = true;
            }
        }
        if let Some(interval) = arguments
            .get("email_poll_interval")
            .and_then(|v| v.as_u64())
        {
            if daemon_config.email_poll_interval_secs != interval {
                daemon_config.email_poll_interval_secs = interval;
                config_changed = true;
            }
        }
        if let Some(interval) = arguments
            .get("calendar_poll_interval")
            .and_then(|v| v.as_u64())
        {
            if daemon_config.calendar_poll_interval_secs != interval {
                daemon_config.calendar_poll_interval_secs = interval;
                config_changed = true;
            }
        }
        if let Some(max) = arguments
            .get("max_concurrent_fetches")
            .and_then(|v| v.as_u64())
        {
            if daemon_config.max_concurrent_fetches != max as usize {
                daemon_config.max_concurrent_fetches = max as usize;
                config_changed = true;
            }
        }

        // Save config if changed
        if config_changed {
            daemon_config.save().ok();
        }

        // Check if already running
        if let Some(pid) = self.is_daemon_running() {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("Daemon is already running (PID {})", pid),
                "status": "running",
                "pid": pid,
                "settings": {
                    "logging_enabled": daemon_config.logging_enabled,
                    "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                    "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                    "max_concurrent_fetches": daemon_config.max_concurrent_fetches
                }
            }));
        }

        // Check if launchd is managing the daemon
        let launchd_installed = DaemonConfig::is_launchd_installed();

        if launchd_installed {
            // Use launchctl to start
            let plist_path = DaemonConfig::launchd_plist_path();
            let output = Command::new("launchctl")
                .args(["load", "-w", plist_path.to_str().unwrap_or("")])
                .output()
                .map_err(|e| Error::Other(format!("Failed to load launchd agent: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Ignore "service already loaded" - just means we need to kickstart
                if !stderr.contains("service already loaded") {
                    return Err(Error::Other(format!(
                        "Failed to start daemon via launchctl: {}",
                        stderr
                    )));
                }
            }

            // Wait for daemon to start
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            if let Some(pid) = self.is_daemon_running() {
                return Ok(serde_json::json!({
                    "success": true,
                    "message": "Daemon started successfully via launchctl",
                    "status": "running",
                    "pid": pid,
                    "settings": {
                        "logging_enabled": daemon_config.logging_enabled,
                        "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                        "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                        "max_concurrent_fetches": daemon_config.max_concurrent_fetches
                    },
                    "log_file": if daemon_config.logging_enabled { Some("~/.local/share/groundeffect/logs/daemon.log") } else { None }
                }));
            } else {
                return Err(Error::Other(
                    "Daemon failed to start via launchctl. Check logs for errors.".to_string(),
                ));
            }
        }

        // No launchd - start daemon directly
        // Get daemon binary path
        let daemon_path = self.get_daemon_binary_path()?;

        // Get credentials from environment (they should be set in the MCP wrapper script)
        let client_id = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_ID")
            .or_else(|_| std::env::var("GROUNDEFFECT_CLIENT_ID"))
            .ok();
        let client_secret = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_SECRET")
            .or_else(|_| std::env::var("GROUNDEFFECT_CLIENT_SECRET"))
            .ok();

        // Start daemon as background process
        let mut cmd = Command::new(&daemon_path);

        // Add logging flag if enabled
        if daemon_config.logging_enabled {
            cmd.arg("--log");
        }

        // Pass through OAuth credentials if available
        if let Some(id) = &client_id {
            cmd.env("GROUNDEFFECT_GOOGLE_CLIENT_ID", id);
        }
        if let Some(secret) = &client_secret {
            cmd.env("GROUNDEFFECT_GOOGLE_CLIENT_SECRET", secret);
        }

        // Pass settings via environment variables
        cmd.env(
            "GROUNDEFFECT_EMAIL_POLL_INTERVAL",
            daemon_config.email_poll_interval_secs.to_string(),
        );
        cmd.env(
            "GROUNDEFFECT_CALENDAR_POLL_INTERVAL",
            daemon_config.calendar_poll_interval_secs.to_string(),
        );
        cmd.env(
            "GROUNDEFFECT_MAX_CONCURRENT_FETCHES",
            daemon_config.max_concurrent_fetches.to_string(),
        );

        // Spawn the daemon
        let child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::Other(format!("Failed to start daemon: {}", e)))?;

        let pid = child.id();

        // Write PID file
        let pid_file = self.config.daemon_pid_file();
        if let Some(parent) = pid_file.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&pid_file, pid.to_string())
            .map_err(|e| Error::Other(format!("Failed to write PID file: {}", e)))?;

        // Wait a moment for daemon to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Verify it's running
        if self.is_daemon_running().is_some() {
            Ok(serde_json::json!({
                "success": true,
                "message": "Daemon started successfully",
                "status": "running",
                "pid": pid,
                "settings": {
                    "logging_enabled": daemon_config.logging_enabled,
                    "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                    "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                    "max_concurrent_fetches": daemon_config.max_concurrent_fetches
                },
                "log_file": if daemon_config.logging_enabled { Some("~/.local/share/groundeffect/logs/daemon.log") } else { None }
            }))
        } else {
            // Clean up PID file if daemon didn't start
            let _ = std::fs::remove_file(&pid_file);
            Err(Error::Other(
                "Daemon started but exited immediately. Check logs for errors.".to_string(),
            ))
        }
    }

    /// Stop the daemon
    async fn daemon_stop(&self) -> Result<Value> {
        let pid = match self.is_daemon_running() {
            Some(pid) => pid,
            None => {
                return Ok(serde_json::json!({
                    "success": true,
                    "message": "Daemon is not running",
                    "status": "stopped"
                }));
            }
        };

        // Check if launchd is managing the daemon
        let launchd_installed = DaemonConfig::is_launchd_installed();

        if launchd_installed {
            // Use launchctl to stop - need to unload to prevent auto-restart from KeepAlive
            let plist_path = DaemonConfig::launchd_plist_path();
            let output = Command::new("launchctl")
                .args(["unload", plist_path.to_str().unwrap_or("")])
                .output()
                .map_err(|e| Error::Other(format!("Failed to unload launchd agent: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Ignore "could not find service" - means it's already unloaded
                if !stderr.contains("Could not find specified service") {
                    return Err(Error::Other(format!(
                        "Failed to stop daemon via launchctl: {}",
                        stderr
                    )));
                }
            }
        } else {
            // No launchd - send SIGTERM directly
            #[cfg(unix)]
            {
                let output = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .output()
                    .map_err(|e| Error::Other(format!("Failed to send stop signal: {}", e)))?;

                if !output.status.success() {
                    return Err(Error::Other(format!(
                        "Failed to stop daemon: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
            }
        }

        // Wait for daemon to stop
        for _ in 0..10 {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            if self.is_daemon_running().is_none() {
                break;
            }
        }

        // Clean up PID file
        let pid_file = self.config.daemon_pid_file();
        let _ = std::fs::remove_file(&pid_file);

        if self.is_daemon_running().is_none() {
            Ok(serde_json::json!({
                "success": true,
                "message": "Daemon stopped successfully",
                "status": "stopped"
            }))
        } else {
            // Force kill if still running
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .output();
            }

            Ok(serde_json::json!({
                "success": true,
                "message": "Daemon force stopped",
                "status": "stopped"
            }))
        }
    }

    /// Restart the daemon
    async fn daemon_restart(&self, arguments: &Value) -> Result<Value> {
        // Check if launchd is managing the daemon
        let launchd_installed = DaemonConfig::is_launchd_installed();

        if launchd_installed {
            // Use launchctl for atomic restart - unload then load
            let plist_path = DaemonConfig::launchd_plist_path();

            // Unload (stops the daemon)
            let _ = Command::new("launchctl")
                .args(["unload", plist_path.to_str().unwrap_or("")])
                .output();

            // Brief pause
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Load (starts the daemon)
            let output = Command::new("launchctl")
                .args(["load", "-w", plist_path.to_str().unwrap_or("")])
                .output()
                .map_err(|e| Error::Other(format!("Failed to load launchd agent: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("service already loaded") {
                    return Err(Error::Other(format!(
                        "Failed to restart daemon via launchctl: {}",
                        stderr
                    )));
                }
            }

            // Wait for daemon to start
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let daemon_config = DaemonConfig::load().unwrap_or_default();
            if let Some(pid) = self.is_daemon_running() {
                return Ok(serde_json::json!({
                    "success": true,
                    "message": "Daemon restarted successfully via launchctl",
                    "status": "running",
                    "pid": pid,
                    "settings": {
                        "logging_enabled": daemon_config.logging_enabled,
                        "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                        "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                        "max_concurrent_fetches": daemon_config.max_concurrent_fetches
                    }
                }));
            } else {
                return Err(Error::Other(
                    "Daemon failed to restart via launchctl".to_string(),
                ));
            }
        }

        // No launchd - use direct process management
        // Stop if running
        if self.is_daemon_running().is_some() {
            self.daemon_stop().await?;
            // Brief pause to ensure clean shutdown
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        // Start with provided arguments
        self.daemon_start(arguments).await
    }

    /// Get daemon status
    async fn daemon_status(&self) -> Result<Value> {
        // Load daemon config
        let daemon_config = DaemonConfig::load().unwrap_or_default();
        let launchd_installed = DaemonConfig::is_launchd_installed();

        match self.is_daemon_running() {
            Some(pid) => {
                // Get additional info about the daemon process
                let mut process_info = serde_json::json!({
                    "running": true,
                    "pid": pid,
                    "status": "running",
                    "settings": {
                        "logging_enabled": daemon_config.logging_enabled,
                        "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                        "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                        "max_concurrent_fetches": daemon_config.max_concurrent_fetches
                    },
                    "launchd_agent_installed": launchd_installed,
                    "log_file": if daemon_config.logging_enabled { Some("~/.local/share/groundeffect/logs/daemon.log") } else { None }
                });

                // Try to get process uptime on Unix
                #[cfg(unix)]
                {
                    if let Ok(output) = Command::new("ps")
                        .args(["-o", "etime=", "-p", &pid.to_string()])
                        .output()
                    {
                        if output.status.success() {
                            let uptime = String::from_utf8_lossy(&output.stdout).trim().to_string();
                            if !uptime.is_empty() {
                                process_info["uptime"] = serde_json::json!(uptime);
                            }
                        }
                    }
                }

                Ok(process_info)
            }
            None => Ok(serde_json::json!({
                "running": false,
                "status": "stopped",
                "message": "Daemon is not running. Use manage_daemon with action: 'start' to start it.",
                "settings": {
                    "logging_enabled": daemon_config.logging_enabled,
                    "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                    "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                    "max_concurrent_fetches": daemon_config.max_concurrent_fetches
                },
                "launchd_agent_installed": launchd_installed
            })),
        }
    }
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
