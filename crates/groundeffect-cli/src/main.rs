//! GroundEffect CLI
//!
//! Full-featured command-line interface for managing and querying GroundEffect.

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand};
use serde::Serialize;

use groundeffect_core::config::{Config, DaemonConfig, EmbeddingFallback};
use groundeffect_core::db::Database;
use groundeffect_core::embedding::{EmbeddingEngine, EmbeddingModel, HybridEmbeddingProvider};
use groundeffect_core::models::{Account, AccountStatus, CalendarEvent, Email, EventTime};
use groundeffect_core::oauth::OAuthManager;
use groundeffect_core::search::{CalendarSearchOptions, SearchEngine, SearchOptions};
use groundeffect_core::token_provider::create_token_provider;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

/// Attendee detail for JSON calendar output
#[derive(Serialize)]
struct AttendeeDetail {
    email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_status: Option<String>,
    optional: bool,
}

#[derive(Parser)]
#[command(name = "groundeffect")]
#[command(about = "GroundEffect - Local email and calendar sync with semantic search")]
#[command(long_about = "GroundEffect syncs Gmail and Google Calendar to a local database with full-text \
and semantic (vector) search capabilities. Data is stored locally in LanceDB.

QUICK START:
  1. Start daemon:     groundeffect daemon start
  2. Check status:     groundeffect sync status
  3. Search emails:    groundeffect email search \"meeting notes\"
  4. Search calendar:  groundeffect calendar search \"standup\"

OUTPUT FORMAT:
  All commands output JSON by default (best for programmatic/AI use).
  Add --human only for direct terminal reading by humans.

JSON RESPONSE FIELDS:
  sync status returns: account, status, email_count, event_count, oldest_email, newest_email,
    oldest_event, newest_event, last_email_sync, last_calendar_sync, attachments_total,
    attachments_downloaded, attachments_size_bytes, sync_email_since, sync_attachments

  account show returns: email, alias, display_name, status, added_at, email_count, event_count,
    attachments_total, attachments_downloaded, attachments_size_bytes, last_sync_email,
    last_sync_calendar, sync_email_since, sync_attachments

  email search returns: id, from, to, subject, date, snippet, account_id, score

KEY SETTINGS:
  - sync_email_since: ISO 8601 date - emails before this date are not synced
  - sync_attachments: boolean - whether attachment auto-download is enabled
  - oldest_email: actual oldest email date in database (may differ from sync_email_since)

MODIFYING SETTINGS (via MCP/Claude Code skill):
  - To sync older emails: manage_sync action='extend' target_date='YYYY-MM-DD'
  - To enable attachments: manage_accounts action='configure' sync_attachments=true
  - To set alias: manage_accounts action='configure' alias='myalias'

EXAMPLES:
  groundeffect email search \"budget report\" --from finance@company.com --after 2024-01-01
  groundeffect calendar search \"team meeting\" --after 2024-06-01 --limit 20
  groundeffect account show user@gmail.com
  groundeffect sync status")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output in human-readable format instead of JSON. Applies to all subcommands.
    #[arg(long, global = true)]
    human: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Search, list, and view emails. Use 'email search' for semantic search across all synced emails.
    Email {
        #[command(subcommand)]
        command: EmailCommands,
    },
    /// Search and view calendar events. Use 'calendar search' for semantic search across events.
    Calendar {
        #[command(subcommand)]
        command: CalendarCommands,
    },
    /// View account details including sync settings (sync_email_since, sync_attachments).
    Account {
        #[command(subcommand)]
        command: AccountCommands,
    },
    /// Check sync status: email/event counts, date ranges, and configured sync settings.
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },
    /// Start, stop, or check status of the background sync daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
    /// Configure groundeffect settings and Claude Code integration.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

// ============================================================================
// Email Commands
// ============================================================================

#[derive(Subcommand)]
enum EmailCommands {
    /// Search emails using hybrid BM25 + vector semantic search.
    /// Returns JSON array with: id, from, to, subject, date, snippet, account_id, score.
    /// Use the 'id' field with 'email show' to get full email body.
    #[command(long_about = "Search emails using hybrid BM25 + semantic vector search.

Returns JSON array of matching emails, sorted by relevance score.

RESPONSE FIELDS:
  id          - Unique email ID (use with 'email show' for full content)
  from        - Sender email address
  to          - Array of recipient email addresses
  subject     - Email subject line
  date        - ISO 8601 timestamp
  snippet     - Preview of email body (first ~100 chars)
  account_id  - Which synced account this email belongs to
  score       - Relevance score (higher = better match)

SEARCH TIPS:
  - Query uses semantic search: \"budget discussions\" finds related emails even without exact words
  - Combine with filters for precise results: --from, --after, --before
  - Date format is YYYY-MM-DD

EXAMPLES:
  groundeffect email search \"quarterly budget\"
  groundeffect email search \"project status\" --from manager@company.com
  groundeffect email search \"invoice\" --after 2024-01-01 --has-attachment")]
    Search {
        /// Natural language search query. Uses semantic search - finds conceptually similar content.
        query: String,
        /// Filter by sender email address or name (partial match supported)
        #[arg(long)]
        from: Option<String>,
        /// Filter by recipient email address (partial match supported)
        #[arg(long)]
        to: Option<String>,
        /// Only emails after this date (format: YYYY-MM-DD)
        #[arg(long)]
        after: Option<String>,
        /// Only emails before this date (format: YYYY-MM-DD)
        #[arg(long)]
        before: Option<String>,
        /// Filter by Gmail label or IMAP folder name
        #[arg(long)]
        folder: Option<String>,
        /// Only return emails that have attachments
        #[arg(long)]
        has_attachment: bool,
        /// Filter to specific account(s) by email address. Can specify multiple.
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Maximum number of results to return (default: 10, max: 100)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// List most recent emails by date (no search query, just chronological).
    /// Returns same JSON format as search.
    List {
        /// Filter to specific account by email address
        #[arg(long)]
        account: Option<String>,
        /// Maximum number of results (default: 10, max: 100)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Show full email content by ID. Returns: id, from, to, cc, subject, date, folder, account_id, body, thread_id, attachments.
    Show {
        /// Email ID (from search/list results)
        id: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Show all emails in a thread by Gmail thread ID.
    Thread {
        /// Gmail thread ID (numeric, from email show result's thread_id field)
        thread_id: String,
        /// Filter to specific account(s)
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Send an email via Gmail API.
    /// Returns JSON: {status: "preview"|"sent"|"draft_created", email: {...}, message_id?: string}.
    #[command(long_about = "Send an email via Gmail API.

This command works in stages for safety:
  1. Without --confirm or --save-as-draft: Returns a preview of the email for review
  2. With --confirm: Actually sends the email
  3. With --save-as-draft: Saves as draft instead of sending (returns draft_id)

HTML SUPPORT:
  Emails are automatically formatted as HTML if the body contains:
  - Markdown links: [text](url)
  - Plain URLs: https://... or http://...
  - Bold: **text** or __text__
  - Italic: *text* or _text_
  - HTML tags: <b>, <a>, <br>, etc.

  Use --html to force HTML format even without these markers.
  HTML emails are sent as multipart/alternative with both HTML and plain text.

REQUIRED PARAMETERS:
  --from <account>   Account to send from (email or alias)
  --to <emails>      Recipient(s) - can specify multiple times
  --subject <text>   Email subject line
  --body <text>      Email body (plain text, markdown, or HTML)

OPTIONAL PARAMETERS:
  --cc <emails>      CC recipients - can specify multiple times
  --bcc <emails>     BCC recipients - can specify multiple times
  --reply-to <id>    Email ID to reply to (sets In-Reply-To/References headers)
  --html             Force HTML format (auto-detected by default)
  --save-as-draft    Save as draft instead of sending
  --confirm          Actually send (without this, returns preview only)

EXAMPLES:
  # Preview an email
  groundeffect email send --from work --to alice@example.com --subject \"Meeting\" --body \"See you at 3pm\"

  # Send after review
  groundeffect email send --from work --to alice@example.com --subject \"Meeting\" --body \"See you at 3pm\" --confirm

  # Send with HTML (auto-detected from markdown link)
  groundeffect email send --from work --to bob@example.com --subject \"Check this\" \\
    --body \"See [this link](https://example.com)\" --confirm

  # Save as draft instead of sending
  groundeffect email send --from work --to alice@example.com --subject \"Draft\" --body \"Content\" --save-as-draft

  # Reply to an existing email
  groundeffect email send --from work --to bob@example.com --reply-to 18abc123 --subject \"Re: Question\" --body \"Yes\" --confirm")]
    Send {
        /// Account to send from (email or alias)
        #[arg(long)]
        from: String,
        /// Recipient email address(es)
        #[arg(long, required = true)]
        to: Vec<String>,
        /// Email subject
        #[arg(long)]
        subject: String,
        /// Email body (plain text, markdown, or HTML)
        #[arg(long)]
        body: String,
        /// CC recipients
        #[arg(long)]
        cc: Option<Vec<String>>,
        /// BCC recipients
        #[arg(long)]
        bcc: Option<Vec<String>>,
        /// Email ID to reply to
        #[arg(long)]
        reply_to: Option<String>,
        /// Force HTML format (auto-detected by default based on content)
        #[arg(long)]
        html: bool,
        /// Save as draft instead of sending (returns draft_id)
        #[arg(long)]
        save_as_draft: bool,
        /// Confirm and send (without this, returns preview only)
        #[arg(long)]
        confirm: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Get an email attachment content or path.
    /// Returns JSON: {filename, mime_type, size, content?|path?, downloaded}.
    #[command(long_about = "Get an email attachment by email ID and filename or attachment ID.

If the attachment has been downloaded, returns the local file path (for binary files)
or the content directly (for text files like .txt, .csv, .json, etc).

If the attachment hasn't been downloaded, returns an error with instructions
to enable sync_attachments or use 'sync download-attachments'.

REQUIRED PARAMETERS:
  <email_id>         Email ID containing the attachment

OPTIONAL PARAMETERS (one required):
  --filename <name>  Attachment filename (case-insensitive match)
  --attachment-id    Attachment ID (from email show output)

EXAMPLES:
  groundeffect email attachment 18abc123 --filename report.pdf
  groundeffect email attachment 18abc123 --attachment-id att_456")]
    Attachment {
        /// Email ID containing the attachment
        email_id: String,
        /// Attachment filename
        #[arg(long)]
        filename: Option<String>,
        /// Attachment ID
        #[arg(long)]
        attachment_id: Option<String>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// List Gmail folders/labels.
    /// Returns JSON: {folders: [...]}.
    #[command(long_about = "List available Gmail folders/labels.

Returns the standard Gmail system folders. Custom labels are not yet supported.

RESPONSE:
  folders - Array of folder names like \"INBOX\", \"[Gmail]/Sent Mail\", etc.

EXAMPLES:
  groundeffect email folders")]
    Folders {
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Manage email drafts (create, list, show, update, send, delete).
    Draft {
        #[command(subcommand)]
        command: DraftCommands,
    },
}

// ============================================================================
// Draft Commands
// ============================================================================

#[derive(Subcommand)]
enum DraftCommands {
    /// Create a new draft directly (no preview/confirm flow).
    #[command(long_about = "Create a new email draft directly via Gmail API.

Unlike 'email send', this creates a draft immediately without a preview step.
Use 'email draft send' to send the draft later.

HTML SUPPORT:
  Same as 'email send' - auto-detects markdown/URLs or use --html flag.

EXAMPLES:
  groundeffect email draft create --from work --to alice@example.com --subject \"Draft\" --body \"Content\"
  groundeffect email draft create --from me --to bob@example.com --subject \"HTML\" --body \"**Bold**\" --html")]
    Create {
        /// Account to create draft from (email or alias)
        #[arg(long)]
        from: String,
        /// Recipient email address(es)
        #[arg(long, required = true)]
        to: Vec<String>,
        /// Email subject
        #[arg(long)]
        subject: String,
        /// Email body
        #[arg(long)]
        body: String,
        /// CC recipients
        #[arg(long)]
        cc: Option<Vec<String>>,
        /// BCC recipients
        #[arg(long)]
        bcc: Option<Vec<String>>,
        /// Force HTML format
        #[arg(long)]
        html: bool,
        /// Email ID to reply to
        #[arg(long)]
        reply_to: Option<String>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// List all drafts for an account.
    #[command(long_about = "List email drafts from Gmail API.

Drafts are fetched directly from Gmail (not stored locally).

RESPONSE FIELDS:
  draft_id  - Draft ID (use with other draft commands)
  subject   - Draft subject
  to        - Recipients
  snippet   - Preview of body content
  date      - When draft was created/updated

EXAMPLES:
  groundeffect email draft list --from work
  groundeffect email draft list --from me --limit 50")]
    List {
        /// Account to list drafts from (email or alias)
        #[arg(long)]
        from: String,
        /// Maximum number of drafts to return
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Show full content of a specific draft.
    #[command(long_about = "Get full content of a draft by ID.

RESPONSE FIELDS:
  draft_id   - Draft ID
  from       - From address
  to         - Recipients
  cc         - CC recipients
  subject    - Subject line
  body       - Plain text body
  body_html  - HTML body (if available)
  date       - Draft date

EXAMPLES:
  groundeffect email draft show --from work --draft-id r123456")]
    Show {
        /// Account (email or alias)
        #[arg(long)]
        from: String,
        /// Draft ID (from 'draft list')
        #[arg(long)]
        draft_id: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Update an existing draft.
    #[command(long_about = "Update an existing draft.

Only provided fields are updated; omitted fields keep their current values.

EXAMPLES:
  groundeffect email draft update --from work --draft-id r123456 --subject \"New subject\"
  groundeffect email draft update --from me --draft-id r123456 --body \"New content\" --to new@example.com")]
    Update {
        /// Account (email or alias)
        #[arg(long)]
        from: String,
        /// Draft ID to update
        #[arg(long)]
        draft_id: String,
        /// New recipients (replaces existing)
        #[arg(long)]
        to: Option<Vec<String>>,
        /// New subject
        #[arg(long)]
        subject: Option<String>,
        /// New body
        #[arg(long)]
        body: Option<String>,
        /// New CC recipients
        #[arg(long)]
        cc: Option<Vec<String>>,
        /// New BCC recipients
        #[arg(long)]
        bcc: Option<Vec<String>>,
        /// Force HTML format
        #[arg(long)]
        html: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Send an existing draft.
    #[command(long_about = "Send an existing draft by ID.

This sends the draft and removes it from the drafts folder.

EXAMPLES:
  groundeffect email draft send --from work --draft-id r123456")]
    Send {
        /// Account (email or alias)
        #[arg(long)]
        from: String,
        /// Draft ID to send
        #[arg(long)]
        draft_id: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Delete an existing draft.
    #[command(long_about = "Delete a draft by ID.

This permanently removes the draft.

EXAMPLES:
  groundeffect email draft delete --from work --draft-id r123456")]
    Delete {
        /// Account (email or alias)
        #[arg(long)]
        from: String,
        /// Draft ID to delete
        #[arg(long)]
        draft_id: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Calendar Commands
// ============================================================================

#[derive(Subcommand)]
enum CalendarCommands {
    /// Search calendar events using semantic search.
    /// Returns JSON array with: id, summary, start, end, location, account_id, calendar_id, score.
    #[command(long_about = "Search calendar events using semantic vector search.

Returns JSON array of matching events, sorted by relevance score.

RESPONSE FIELDS:
  id          - Unique event ID (use with 'calendar show' for full details)
  summary     - Event title
  start       - Start time (ISO 8601 or YYYY-MM-DD for all-day events)
  end         - End time (ISO 8601 or YYYY-MM-DD for all-day events)
  location    - Event location (may be null)
  account_id  - Which synced account this event belongs to
  calendar_id - Google Calendar ID
  score       - Relevance score (higher = better match)

EXAMPLES:
  groundeffect calendar search \"team standup\"
  groundeffect calendar search \"1:1 meeting\" --after 2024-01-01
  groundeffect calendar search \"quarterly review\" --limit 20")]
    Search {
        /// Natural language search query. Uses semantic search.
        query: String,
        /// Only events starting after this date (format: YYYY-MM-DD)
        #[arg(long)]
        after: Option<String>,
        /// Only events starting before this date (format: YYYY-MM-DD)
        #[arg(long)]
        before: Option<String>,
        /// Filter by Google Calendar ID
        #[arg(long)]
        calendar: Option<String>,
        /// Filter to specific account(s) by email address
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Maximum number of results (default: 10, max: 100)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// List calendars and event counts per account.
    List {
        /// Filter to specific account(s) by email address
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Show full event details by ID. Returns: id, summary, start, end, location, description, attendees, account_id, calendar_id.
    Show {
        /// Event ID (from search results)
        id: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// List calendar events in a date range (no search query required).
    /// Returns JSON array with: id, summary, start, end, location, account_id, calendar_id.
    #[command(long_about = "List calendar events in a date range without semantic search.

Use this command to answer questions like 'what's on my calendar tomorrow' or
'show me my meetings next week'. Unlike 'calendar search', this command does NOT
require a search query - it simply lists all events in the specified date range.

RESPONSE FIELDS:
  id          - Unique event ID (use with 'calendar show' for full details)
  summary     - Event title
  start       - Start time (ISO 8601 or YYYY-MM-DD for all-day events)
  end         - End time (ISO 8601 or YYYY-MM-DD for all-day events)
  location    - Event location (may be null)
  account_id  - Which synced account this event belongs to
  calendar_id - Google Calendar ID

DATE FORMAT:
  Use YYYY-MM-DD format for --from and --to parameters.
  If --from is omitted, defaults to today.
  If --to is omitted, defaults to 7 days after --from.

EXAMPLES:
  # Tomorrow's events
  groundeffect calendar events --from 2024-01-07 --to 2024-01-08

  # Next week's events
  groundeffect calendar events --from 2024-01-06 --to 2024-01-13

  # Events for a specific account
  groundeffect calendar events --from 2024-01-07 --account work@example.com

  # Today's events (default)
  groundeffect calendar events")]
    Events {
        /// Start date (YYYY-MM-DD). Defaults to today if not specified.
        #[arg(long)]
        from: Option<String>,
        /// End date (YYYY-MM-DD). Defaults to 7 days after --from if not specified.
        #[arg(long)]
        to: Option<String>,
        /// Filter to specific account(s) by email address
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Maximum number of results (default: 50, max: 200)
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Create a new calendar event via Google Calendar API.
    /// Returns JSON: {success: bool, event: {id, summary, start, end, html_link}}.
    #[command(long_about = "Create a new calendar event via Google Calendar API.

REQUIRED PARAMETERS:
  --account <email>  Account to create event in (email or alias)
  --summary <text>   Event title
  --start <datetime> Start time in ISO 8601 format (e.g., 2024-01-15T10:00:00)
  --end <datetime>   End time in ISO 8601 format (e.g., 2024-01-15T11:00:00)

OPTIONAL PARAMETERS:
  --description <text>  Event description/notes
  --location <text>     Event location
  --attendees <emails>  Attendee email addresses - can specify multiple times
  --calendar <id>       Calendar ID (default: 'primary')

DATETIME FORMAT:
  Use ISO 8601: YYYY-MM-DDTHH:MM:SS (times are in UTC)
  Example: 2024-01-15T10:00:00

EXAMPLES:
  # Create a simple meeting
  groundeffect calendar create --account work --summary \"Team Standup\" \\
    --start 2024-01-15T09:00:00 --end 2024-01-15T09:30:00

  # Create event with location and attendees
  groundeffect calendar create --account work --summary \"Project Review\" \\
    --start 2024-01-15T14:00:00 --end 2024-01-15T15:00:00 \\
    --location \"Conference Room A\" \\
    --attendees alice@example.com --attendees bob@example.com")]
    Create {
        /// Account to create event in (email or alias)
        #[arg(long)]
        account: String,
        /// Event title
        #[arg(long)]
        summary: String,
        /// Start time (ISO 8601: YYYY-MM-DDTHH:MM:SS)
        #[arg(long)]
        start: String,
        /// End time (ISO 8601: YYYY-MM-DDTHH:MM:SS)
        #[arg(long)]
        end: String,
        /// Event description
        #[arg(long)]
        description: Option<String>,
        /// Event location
        #[arg(long)]
        location: Option<String>,
        /// Attendee email addresses
        #[arg(long)]
        attendees: Option<Vec<String>>,
        /// Calendar ID (default: primary)
        #[arg(long, default_value = "primary")]
        calendar: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Account Commands
// ============================================================================

#[derive(Subcommand)]
enum AccountCommands {
    /// List all synced accounts with status. Returns JSON array with: email, alias, display_name, status, added_at, last_sync_email, last_sync_calendar.
    List {
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Show detailed account info including sync settings.
    /// Returns: email, alias, display_name, status, added_at, email_count, event_count,
    /// attachments_total, attachments_downloaded, attachments_size_bytes, last_sync_email,
    /// last_sync_calendar, sync_email_since, sync_attachments.
    #[command(long_about = "Show detailed account information including sync configuration.

RESPONSE FIELDS:
  email                   - Account email address (primary identifier)
  alias                   - User-defined alias (may be null)
  display_name            - Google profile display name
  status                  - Account status: Active, NeedsReauth, Disabled, or Syncing
  added_at                - When account was added (ISO 8601)
  email_count             - Number of emails synced
  event_count             - Number of calendar events synced
  attachments_total       - Total attachments found in emails
  attachments_downloaded  - Number of attachments downloaded locally
  attachments_size_bytes  - Total size of downloaded attachments
  last_sync_email         - Last successful email sync (ISO 8601, may be null)
  last_sync_calendar      - Last successful calendar sync (ISO 8601, may be null)
  sync_email_since        - Configured email sync start date (ISO 8601, may be null)
  sync_attachments        - Whether attachment auto-download is enabled (boolean)

NOTE: sync_email_since is the configured cutoff - emails older than this are not synced.
Use 'sync status' to see oldest_email which shows the actual oldest email in database.")]
    Show {
        /// Account email address or alias
        account: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Add a new Google account via OAuth. Interactive prompts for sync settings.
    /// Returns JSON: {success: bool, account: {id, alias, display_name, status, years_to_sync}}.
    #[command(long_about = "Add a new Google account via OAuth authentication.

This command will:
  1. Prompt for sync settings (how far back to sync, attachments, alias)
  2. Open your browser to authenticate with Google
  3. Store credentials securely in macOS Keychain
  4. Create the account in the database

PREREQUISITES:
  OAuth credentials must be configured in ~/.secrets:
    export GROUNDEFFECT_GOOGLE_CLIENT_ID=\"your-client-id\"
    export GROUNDEFFECT_GOOGLE_CLIENT_SECRET=\"your-secret\"

  Get credentials from: https://console.cloud.google.com/apis/credentials
  (Create OAuth 2.0 Client ID > Desktop app, enable Gmail & Calendar APIs)

EXAMPLES:
  groundeffect account add
  groundeffect account add --years 5 --alias work")]
    Add {
        /// How many years of email history to sync (1-20, or 'all')
        #[arg(long)]
        years: Option<String>,
        /// Enable automatic attachment downloading
        #[arg(long)]
        attachments: bool,
        /// Alias for the account (e.g., 'work', 'personal')
        #[arg(long)]
        alias: Option<String>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Delete an account and all its synced data.
    /// Returns JSON: {success: bool, deleted: {account, emails, events}}.
    #[command(long_about = "Delete an account and all its synced data.

This will:
  1. Remove all synced emails for this account
  2. Remove all synced calendar events
  3. Remove OAuth tokens from Keychain
  4. Delete the account record

WARNING: This action is irreversible!

EXAMPLES:
  groundeffect account delete user@gmail.com
  groundeffect account delete user@gmail.com --confirm")]
    Delete {
        /// Account email or alias
        account: String,
        /// Confirm deletion (required to proceed)
        #[arg(long)]
        confirm: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Configure account settings (alias, sync_attachments).
    /// Returns JSON: {success: bool, changes: [...], account: {id, alias, sync_attachments}}.
    #[command(long_about = "Configure account settings.

CONFIGURABLE SETTINGS:
  --alias <name>       Set a friendly alias (e.g., 'work', 'personal')
  --alias \"\"           Remove the alias
  --attachments        Enable automatic attachment downloading
  --no-attachments     Disable automatic attachment downloading

Note: Changes to attachment settings require a daemon restart to take effect.

EXAMPLES:
  groundeffect account configure user@gmail.com --alias work
  groundeffect account configure work --attachments
  groundeffect account configure user@gmail.com --alias \"\" --no-attachments")]
    Configure {
        /// Account email or alias
        account: String,
        /// Set alias for the account (use empty string to remove)
        #[arg(long)]
        alias: Option<String>,
        /// Enable automatic attachment downloading
        #[arg(long)]
        attachments: bool,
        /// Disable automatic attachment downloading
        #[arg(long)]
        no_attachments: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Sync Commands
// ============================================================================

#[derive(Subcommand)]
enum SyncCommands {
    /// Show sync status for all accounts or a specific account.
    /// Returns JSON array with comprehensive sync information.
    #[command(long_about = "Show detailed sync status including email/event counts and date ranges.

RESPONSE FIELDS:
  account               - Account email address
  status                - Account status: Active, NeedsReauth, Disabled, or Syncing
  email_count           - Total emails synced
  event_count           - Total calendar events synced
  oldest_email          - Date of oldest email in database (YYYY-MM-DD, may be null)
  newest_email          - Date of newest email in database (YYYY-MM-DD, may be null)
  oldest_event          - Date of oldest event in database (YYYY-MM-DD, may be null)
  newest_event          - Date of newest event in database (YYYY-MM-DD, may be null)
  last_email_sync       - Last email sync time (ISO 8601, may be null)
  last_calendar_sync    - Last calendar sync time (ISO 8601, may be null)
  attachments_total     - Total attachments found
  attachments_downloaded - Attachments downloaded locally
  attachments_size_bytes - Size of downloaded attachments
  sync_email_since      - Configured email sync cutoff (ISO 8601, may be null)
  sync_attachments      - Whether attachment download is enabled (boolean)

IMPORTANT:
  - oldest_email shows actual data range, sync_email_since shows configured limit
  - If oldest_email > sync_email_since, historical sync may still be in progress
  - Check last_email_sync to see when sync last ran

EXAMPLES:
  groundeffect sync status
  groundeffect sync status --account user@gmail.com")]
    Status {
        /// Filter to specific account by email address
        #[arg(long)]
        account: Option<String>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Reset sync data for an account (deletes synced emails/events).
    /// Returns JSON: {success: bool, deleted: {emails, events}}.
    #[command(long_about = "Reset sync data for an account.

WARNING: This permanently deletes synced emails and/or calendar events!

REQUIRED PARAMETERS:
  --account <email>  Account to reset (email or alias)
  --confirm          Required to confirm deletion

OPTIONAL PARAMETERS:
  --data-type <type> What to reset: 'email', 'calendar', or 'all' (default: 'all')

After reset, the daemon will re-sync from the configured sync_email_since date.

EXAMPLES:
  # Reset all data
  groundeffect sync reset --account work --confirm

  # Reset only emails
  groundeffect sync reset --account work --data-type email --confirm")]
    Reset {
        /// Account to reset (email or alias)
        #[arg(long)]
        account: String,
        /// Data type to reset: email, calendar, or all
        #[arg(long, default_value = "all")]
        data_type: String,
        /// Confirm reset (required)
        #[arg(long)]
        confirm: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Extend sync range to include older emails/events.
    /// Returns JSON: {success: bool, sync_range: {previous, new, additional_days}}.
    #[command(long_about = "Extend the sync range to include older data.

This changes the sync_email_since date to an earlier date, allowing
the daemon to sync older emails and events.

REQUIRED PARAMETERS:
  --account <email>     Account to extend (email or alias)
  --target-date <date>  New start date (YYYY-MM-DD format)

The target date must be earlier than the current sync_email_since date.

EXAMPLES:
  # Extend sync to include 2020 data
  groundeffect sync extend --account work --target-date 2020-01-01")]
    Extend {
        /// Account to extend (email or alias)
        #[arg(long)]
        account: String,
        /// Target date to sync from (YYYY-MM-DD)
        #[arg(long)]
        target_date: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Force sync to resume from a specific date.
    /// Returns JSON: {success: bool, resume_from: string, previous_state: {...}}.
    #[command(long_about = "Force sync to resume from a specific date.

This sets the oldest_email_synced/oldest_event_synced to the target date,
causing the daemon to resume historical sync from that point.

Useful if sync was interrupted or you want to re-sync a specific time period.
Existing data is preserved - duplicates are prevented by ID matching.

REQUIRED PARAMETERS:
  --account <email>     Account to modify (email or alias)
  --target-date <date>  Date to resume from (YYYY-MM-DD format)

EXAMPLES:
  # Resume sync from June 2023
  groundeffect sync resume-from --account work --target-date 2023-06-01")]
    #[command(name = "resume-from")]
    ResumeFrom {
        /// Account to modify (email or alias)
        #[arg(long)]
        account: String,
        /// Date to resume sync from (YYYY-MM-DD)
        #[arg(long)]
        target_date: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Download attachments for an account.
    /// Returns JSON: {success: bool, pending_count: number, sync_attachments_enabled: bool}.
    #[command(name = "download-attachments")]
    #[command(long_about = "Enable attachment downloading for an account.

This enables the sync_attachments setting for the account if not already enabled,
and the daemon will download all pending attachments in the background.

REQUIRED PARAMETERS:
  --account <email>  Account to download attachments for (email or alias)

The daemon handles downloads in the background. Restart the daemon if it's
not running to start the download process.

EXAMPLES:
  groundeffect sync download-attachments --account work")]
    DownloadAttachments {
        /// Account to download attachments for (email or alias)
        #[arg(long)]
        account: String,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Daemon Commands
// ============================================================================

#[derive(Subcommand)]
enum DaemonCommands {
    /// Check if daemon is running. Returns JSON: {running: bool, pid: number|null}.
    Status {
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Start the background sync daemon. Uses launchd if plist exists, otherwise direct spawn.
    /// Returns JSON: {status: "started"|"already_running"|"error", method?: "launchd"|"direct"}.
    Start {
        /// Enable file logging (only for direct spawn, not launchd)
        #[arg(long)]
        logging: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Stop the daemon. Returns JSON: {status: "stopped"|"not_running"}.
    Stop {
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Restart the daemon (stop then start). Returns JSON: {status: "restarted", method: "launchd"|"direct"}.
    Restart {
        /// Enable file logging (only for direct spawn, not launchd)
        #[arg(long)]
        logging: bool,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Install launchd agent for auto-start at login. Uses smart defaults (no prompts).
    /// Returns JSON: {status: "installed"|"already_installed"|"error"}.
    #[command(long_about = "Install the launchd agent for automatic daemon startup at login.

Uses sensible defaults:
  - Logging: disabled (logs to ~/.local/share/groundeffect/logs/ when enabled)
  - Email poll interval: 300 seconds (5 minutes)
  - Calendar poll interval: 300 seconds (5 minutes)
  - Max concurrent fetches: 10

The daemon will start automatically after installation and on every login.

FILES CREATED:
  ~/Library/LaunchAgents/com.groundeffect.daemon.plist
  ~/.config/groundeffect/daemon.toml

TO CUSTOMIZE SETTINGS:
  groundeffect config settings

EXAMPLES:
  groundeffect daemon install
  groundeffect daemon install --logging true
  groundeffect daemon install --logging false")]
    Install {
        /// Enable file logging to ~/.local/share/groundeffect/logs/
        /// If not specified, preserves existing config or defaults to false.
        #[arg(long)]
        logging: Option<bool>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Uninstall launchd agent. Stops the daemon and removes auto-start.
    /// Returns JSON: {status: "uninstalled"|"not_installed"}.
    #[command(long_about = "Uninstall the launchd agent and stop automatic daemon startup.

This will:
  1. Stop the running daemon (if any)
  2. Remove the launchd plist from ~/Library/LaunchAgents/
  3. The daemon will no longer start automatically at login

Note: This does NOT remove synced data or configuration files.

EXAMPLES:
  groundeffect daemon uninstall")]
    Uninstall {
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Config Commands
// ============================================================================

#[derive(Subcommand)]
enum ConfigCommands {
    /// Add groundeffect to Claude Code's allowed commands (~/.claude/settings.json).
    #[command(name = "add-permissions")]
    #[command(long_about = "Add groundeffect to Claude Code's command allowlist.

This modifies ~/.claude/settings.json to allow Claude Code to run all groundeffect
commands without prompting for permission each time.

WHAT IT DOES:
  Adds 'Bash(groundeffect:*)' to the 'allow' list in your Claude Code settings.

FILE MODIFIED:
  ~/.claude/settings.json

EXAMPLE:
  groundeffect config add-permissions

After running this, Claude Code can use groundeffect for email/calendar tasks
without asking for permission on each command.")]
    AddPermissions,
    /// Remove groundeffect from Claude Code's allowed commands.
    #[command(name = "remove-permissions")]
    #[command(long_about = "Remove groundeffect from Claude Code's command allowlist.

This modifies ~/.claude/settings.json to remove groundeffect from the allowed
commands, requiring permission prompts again.

WHAT IT DOES:
  Removes 'Bash(groundeffect:*)' from the 'allow' list in your Claude Code settings.

FILE MODIFIED:
  ~/.claude/settings.json

EXAMPLE:
  groundeffect config remove-permissions")]
    RemovePermissions,
    /// View or modify daemon settings interactively.
    #[command(long_about = "View or modify daemon settings.

Without flags, shows current settings. Use flags to modify specific settings.

CONFIGURABLE SETTINGS:
  --logging <bool>           Enable/disable file logging
  --email-interval <secs>    Email poll interval (60-3600 seconds)
  --calendar-interval <secs> Calendar poll interval (60-3600 seconds)
  --max-fetches <num>        Max concurrent fetches (1-50)
  --timezone <tz>            User timezone (e.g., America/Los_Angeles, UTC)

CONFIG FILES:
  ~/.config/groundeffect/daemon.toml (daemon settings)
  ~/.config/groundeffect/config.toml (general settings including timezone)

Note: Changes require a daemon restart to take effect.

EXAMPLES:
  # Show current settings
  groundeffect config settings

  # Enable logging
  groundeffect config settings --logging true

  # Set poll intervals
  groundeffect config settings --email-interval 600 --calendar-interval 600")]
    Settings {
        /// Enable/disable file logging
        #[arg(long)]
        logging: Option<bool>,
        /// Email poll interval in seconds (60-3600)
        #[arg(long)]
        email_interval: Option<u64>,
        /// Calendar poll interval in seconds (60-3600)
        #[arg(long)]
        calendar_interval: Option<u64>,
        /// Max concurrent fetches (1-50)
        #[arg(long)]
        max_fetches: Option<u32>,
        /// User timezone (e.g., America/Los_Angeles, UTC, Europe/London)
        #[arg(long)]
        timezone: Option<String>,
        /// Human-readable output instead of JSON
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// JSON Output Types
// ============================================================================

#[derive(Serialize)]
struct EmailResult {
    id: String,
    from: String,
    to: Vec<String>,
    subject: String,
    date: String,
    folder: String,
    account_id: String,
    snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f32>,
}

impl EmailResult {
    fn from_email(email: &Email, score: Option<f32>) -> Self {
        Self {
            id: email.id.clone(),
            from: email.from.to_string(),
            to: email.to.iter().map(|a| a.to_string()).collect(),
            subject: email.subject.clone(),
            date: email.date.to_rfc3339(),
            folder: email.folder.clone(),
            account_id: email.account_id.clone(),
            snippet: email.snippet.clone(),
            score,
        }
    }
}

#[derive(Serialize)]
struct EmailDetail {
    id: String,
    from: String,
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    date: String,
    folder: String,
    account_id: String,
    body: String,
    thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<AttachmentInfo>>,
}

#[derive(Serialize)]
struct AttachmentInfo {
    id: String,
    filename: String,
    mime_type: String,
    size: u64,
}

impl EmailDetail {
    fn from_email(email: &Email) -> Self {
        let attachments = if email.attachments.is_empty() {
            None
        } else {
            Some(
                email
                    .attachments
                    .iter()
                    .map(|a| AttachmentInfo {
                        id: a.id.clone(),
                        filename: a.filename.clone(),
                        mime_type: a.mime_type.clone(),
                        size: a.size,
                    })
                    .collect(),
            )
        };
        Self {
            id: email.id.clone(),
            from: email.from.to_string(),
            to: email.to.iter().map(|a| a.to_string()).collect(),
            cc: email.cc.iter().map(|a| a.to_string()).collect(),
            subject: email.subject.clone(),
            date: email.date.to_rfc3339(),
            folder: email.folder.clone(),
            account_id: email.account_id.clone(),
            body: email.body_plain.clone(),
            thread_id: email.gmail_thread_id.to_string(),
            attachments,
        }
    }
}

#[derive(Serialize)]
struct EventResult {
    id: String,
    summary: String,
    start: String,
    end: String,
    location: Option<String>,
    account_id: String,
    calendar_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f32>,
}

impl EventResult {
    fn from_event(event: &CalendarEvent, score: Option<f32>) -> Self {
        Self {
            id: event.id.clone(),
            summary: event.summary.clone(),
            start: format_event_time(&event.start),
            end: format_event_time(&event.end),
            location: event.location.clone(),
            account_id: event.account_id.clone(),
            calendar_id: event.calendar_id.clone(),
            score,
        }
    }
}

fn format_event_time(et: &EventTime) -> String {
    match et {
        EventTime::DateTime(dt) => dt.to_rfc3339(),
        EventTime::Date(d) => d.to_string(),
    }
}

#[derive(Serialize)]
struct AccountResult {
    email: String,
    alias: Option<String>,
    display_name: String,
    status: String,
    added_at: String,
    last_sync_email: Option<String>,
    last_sync_calendar: Option<String>,
}

impl AccountResult {
    fn from_account(account: &Account) -> Self {
        Self {
            email: account.id.clone(),
            alias: account.alias.clone(),
            display_name: account.display_name.clone(),
            status: format!("{:?}", account.status),
            added_at: account.added_at.to_rfc3339(),
            last_sync_email: account.last_sync_email.map(|d| d.to_rfc3339()),
            last_sync_calendar: account.last_sync_calendar.map(|d| d.to_rfc3339()),
        }
    }
}

#[derive(Serialize)]
struct SyncStatus {
    account: String,
    status: String,
    email_count: u64,
    event_count: u64,
    oldest_email: Option<String>,
    newest_email: Option<String>,
    oldest_event: Option<String>,
    newest_event: Option<String>,
    last_email_sync: Option<String>,
    last_calendar_sync: Option<String>,
    attachments_total: usize,
    attachments_downloaded: usize,
    attachments_size_bytes: u64,
    sync_email_since: Option<String>,
    sync_attachments: bool,
    estimated_total_emails: Option<u64>,
    emails_remaining: Option<u64>,
}

#[derive(Serialize)]
struct DaemonStatus {
    running: bool,
    pid: Option<u32>,
}

// ============================================================================
// Main Entry Point
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let global_human = cli.human;

    match cli.command {
        Commands::Email { command } => handle_email_command(command, global_human).await,
        Commands::Calendar { command } => handle_calendar_command(command, global_human).await,
        Commands::Account { command } => handle_account_command(command, global_human).await,
        Commands::Sync { command } => handle_sync_command(command, global_human).await,
        Commands::Daemon { command } => handle_daemon_command(command, global_human).await,
        Commands::Config { command } => handle_config_command(command).await,
    }
}

// ============================================================================
// Email Command Handlers
// ============================================================================

async fn handle_email_command(command: EmailCommands, global_human: bool) -> Result<()> {
    match command {
        EmailCommands::Search {
            query,
            from,
            to,
            after,
            before,
            folder,
            has_attachment,
            account,
            limit,
            human,
        } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Arc::new(Database::open(config.lancedb_dir()).await?);

            // Initialize embedding engine with hybrid remote/local support
            // Skip loading local model if using remote with BM25 fallback (saves CPU/memory)
            let local_embedding = if config.search.embedding_url.is_some()
                && config.search.embedding_fallback == EmbeddingFallback::Bm25
            {
                None
            } else {
                let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
                    .unwrap_or(EmbeddingModel::BgeBaseEn);
                Some(Arc::new(
                    EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_gpu)?
                ))
            };
            let embedding = Arc::new(HybridEmbeddingProvider::new(
                local_embedding,
                config.search.embedding_url.clone(),
                config.search.embedding_timeout_ms,
                config.search.embedding_fallback,
            )?);

            let search_engine = SearchEngine::new(db.clone(), embedding);

            // Resolve account aliases to IDs
            let accounts = if let Some(accts) = account {
                let all_accounts = db.list_accounts().await?;
                let resolved: Vec<String> = accts
                    .iter()
                    .filter_map(|a| resolve_account(&all_accounts, a))
                    .collect();
                if resolved.is_empty() { None } else { Some(resolved) }
            } else {
                None
            };

            let mut options = SearchOptions::new(limit.min(100));
            options.accounts = accounts;
            options.folder = folder;
            options.from = from;
            options.to = to;
            options.date_from = parse_date(&after, &config.general.timezone);
            options.date_to = parse_date(&before, &config.general.timezone);
            options.has_attachment = if has_attachment { Some(true) } else { None };

            let results = search_engine.search_emails(&query, &options).await?;

            if human {
                if results.is_empty() {
                    println!("No emails found.");
                } else {
                    println!("\nFound {} emails:\n", results.len());
                    for result in &results {
                        let e = &result.email;
                        println!("📧 {} (score: {:.2})", e.subject, result.score);
                        println!("   From: {}", e.from);
                        println!("   Date: {}", e.date.format("%Y-%m-%d %H:%M"));
                        println!("   ID: {}", e.id);
                        println!();
                    }
                }
            } else {
                let json_results: Vec<serde_json::Value> = results
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.email.id,
                            "from": r.email.from.to_string(),
                            "to": r.email.to.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                            "subject": r.email.subject,
                            "date": r.email.date.to_rfc3339(),
                            "snippet": r.email.snippet,
                            "account_id": r.email.account_id,
                            "score": r.score
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_results)?);
            }
        }

        EmailCommands::List { account, limit, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;

            let account_id = if let Some(acct) = account {
                let all_accounts = db.list_accounts().await?;
                resolve_account(&all_accounts, &acct)
            } else {
                None
            };

            let emails = db.list_recent_emails(account_id.as_deref(), limit.min(100)).await?;

            if human {
                if emails.is_empty() {
                    println!("No emails found.");
                } else {
                    println!("\nRecent {} emails:\n", emails.len());
                    for email in &emails {
                        println!("📧 {}", email.subject);
                        println!("   From: {}", email.from);
                        println!("   Date: {}", email.date.format("%Y-%m-%d %H:%M"));
                        println!("   ID: {}", email.id);
                        println!();
                    }
                }
            } else {
                let json_results: Vec<EmailResult> = emails
                    .iter()
                    .map(|e| EmailResult::from_email(e, None))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_results)?);
            }
        }

        EmailCommands::Show { id, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;

            match db.get_email(&id).await? {
                Some(email) => {
                    if human {
                        println!("\n📧 {}", email.subject);
                        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                        println!("From: {}", email.from);
                        println!("To: {}", email.to.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "));
                        if !email.cc.is_empty() {
                            println!("CC: {}", email.cc.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "));
                        }
                        println!("Date: {}", email.date.format("%Y-%m-%d %H:%M:%S"));
                        println!("Folder: {}", email.folder);
                        if !email.attachments.is_empty() {
                            println!("Attachments: {}", email.attachments.iter().map(|a| a.filename.as_str()).collect::<Vec<_>>().join(", "));
                        }
                        println!("\n{}", email.body_plain);
                    } else {
                        let detail = EmailDetail::from_email(&email);
                        println!("{}", serde_json::to_string_pretty(&detail)?);
                    }
                }
                None => {
                    if human {
                        println!("Email not found: {}", id);
                    } else {
                        println!("{{\"error\": \"Email not found\"}}");
                    }
                }
            }
        }

        EmailCommands::Thread { thread_id, account, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;

            // Parse thread_id as u64 (gmail_thread_id)
            let thread_id_num: u64 = thread_id.parse().unwrap_or(0);

            let account_id = if let Some(accts) = account {
                let all_accounts = db.list_accounts().await?;
                accts.first().and_then(|a| resolve_account(&all_accounts, a))
            } else {
                None
            };

            let emails = db.get_emails_by_thread(thread_id_num, account_id.as_deref()).await?;

            if human {
                if emails.is_empty() {
                    println!("No emails found in thread: {}", thread_id);
                } else {
                    println!("\nThread {} ({} emails):\n", thread_id, emails.len());
                    for email in &emails {
                        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                        println!("📧 {}", email.subject);
                        println!("From: {}", email.from);
                        println!("Date: {}", email.date.format("%Y-%m-%d %H:%M"));
                        println!("\n{}\n", email.body_plain.chars().take(500).collect::<String>());
                    }
                }
            } else {
                let json_results: Vec<EmailDetail> = emails
                    .iter()
                    .map(EmailDetail::from_email)
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_results)?);
            }
        }

        EmailCommands::Send { from, to, subject, body, cc, bcc, reply_to, html, save_as_draft, confirm, human } => {
            let human = human || global_human;
            email_send(&from, to, &subject, &body, cc, bcc, reply_to, html, save_as_draft, confirm, human).await?;
        }

        EmailCommands::Draft { command } => {
            handle_draft_command(command, global_human).await?;
        }

        EmailCommands::Attachment { email_id, filename, attachment_id, human } => {
            let human = human || global_human;
            email_attachment(&email_id, filename.as_deref(), attachment_id.as_deref(), human).await?;
        }

        EmailCommands::Folders { human } => {
            let human = human || global_human;
            let folders = vec![
                "INBOX",
                "[Gmail]/All Mail",
                "[Gmail]/Drafts",
                "[Gmail]/Important",
                "[Gmail]/Sent Mail",
                "[Gmail]/Spam",
                "[Gmail]/Starred",
                "[Gmail]/Trash",
            ];

            if human {
                println!("\n📁 Gmail Folders:\n");
                for folder in &folders {
                    println!("  {}", folder);
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "folders": folders
                }))?);
            }
        }
    }

    Ok(())
}

// ============================================================================
// Calendar Command Handlers
// ============================================================================

async fn handle_calendar_command(command: CalendarCommands, global_human: bool) -> Result<()> {
    match command {
        CalendarCommands::Search {
            query,
            after,
            before,
            calendar,
            account,
            limit,
            human,
        } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Arc::new(Database::open(config.lancedb_dir()).await?);

            // Initialize embedding engine with hybrid remote/local support
            // Skip loading local model if using remote with BM25 fallback (saves CPU/memory)
            let local_embedding = if config.search.embedding_url.is_some()
                && config.search.embedding_fallback == EmbeddingFallback::Bm25
            {
                None
            } else {
                let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
                    .unwrap_or(EmbeddingModel::BgeBaseEn);
                Some(Arc::new(
                    EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_gpu)?
                ))
            };
            let embedding = Arc::new(HybridEmbeddingProvider::new(
                local_embedding,
                config.search.embedding_url.clone(),
                config.search.embedding_timeout_ms,
                config.search.embedding_fallback,
            )?);

            let search_engine = SearchEngine::new(db.clone(), embedding);

            // Resolve account aliases
            let accounts = if let Some(accts) = account {
                let all_accounts = db.list_accounts().await?;
                let resolved: Vec<String> = accts
                    .iter()
                    .filter_map(|a| resolve_account(&all_accounts, a))
                    .collect();
                if resolved.is_empty() { None } else { Some(resolved) }
            } else {
                None
            };

            let options = CalendarSearchOptions {
                accounts,
                limit: limit.min(100),
                calendar_id: calendar,
                date_from: parse_date(&after, &config.general.timezone),
                date_to: parse_date(&before, &config.general.timezone),
            };

            let results = search_engine.search_calendar(&query, &options).await?;

            if human {
                if results.is_empty() {
                    println!("No events found.");
                } else {
                    println!("\nFound {} events:\n", results.len());
                    for result in &results {
                        println!("📅 {} (score: {:.2})", result.event.summary, result.score);
                        println!("   When: {} - {}",
                            format_event_time_human(&result.event.start),
                            format_event_time_human(&result.event.end));
                        if let Some(loc) = &result.event.location {
                            println!("   Where: {}", loc);
                        }
                        println!("   ID: {}", result.event.id);
                        println!();
                    }
                }
            } else {
                let json_results: Vec<EventResult> = results
                    .iter()
                    .map(|r| EventResult::from_event(&r.event, Some(r.score)))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_results)?);
            }
        }

        CalendarCommands::List { account, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;
            let accounts = db.list_accounts().await?;

            let _ = account; // Filter not implemented yet

            if human {
                println!("\n📅 Calendars:\n");
                for account in &accounts {
                    println!("Account: {}", account.id);
                    let event_count = db.count_events(Some(&account.id)).await.unwrap_or(0);
                    println!("  Events synced: {}", event_count);
                    println!();
                }
            } else {
                #[derive(Serialize)]
                struct CalendarInfo {
                    account: String,
                    event_count: u64,
                }
                let mut results = Vec::new();
                for account in &accounts {
                    let event_count = db.count_events(Some(&account.id)).await.unwrap_or(0);
                    results.push(CalendarInfo {
                        account: account.id.clone(),
                        event_count,
                    });
                }
                println!("{}", serde_json::to_string_pretty(&results)?);
            }
        }

        CalendarCommands::Show { id, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;

            match db.get_event(&id).await? {
                Some(event) => {
                    if human {
                        println!("\n📅 {}", event.summary);
                        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                        println!("When: {} - {}",
                            format_event_time_human(&event.start),
                            format_event_time_human(&event.end));
                        if let Some(loc) = &event.location {
                            println!("Where: {}", loc);
                        }
                        // Show organizer
                        if let Some(org) = &event.organizer {
                            let org_name = org.name.as_deref().unwrap_or(&org.email);
                            println!("\nOrganizer: {} <{}>", org_name, org.email);
                        }
                        // Show attendees with response status
                        if !event.attendees.is_empty() {
                            println!("\nAttendees ({}):", event.attendees.len());
                            for attendee in &event.attendees {
                                let name = attendee.name.as_deref().unwrap_or(&attendee.email);
                                let status_icon = match &attendee.response_status {
                                    Some(s) => match s {
                                        groundeffect_core::models::AttendeeStatus::Accepted => "✓",
                                        groundeffect_core::models::AttendeeStatus::Declined => "✗",
                                        groundeffect_core::models::AttendeeStatus::Tentative => "?",
                                        groundeffect_core::models::AttendeeStatus::NeedsAction => "?",
                                    },
                                    None => " ",
                                };
                                let optional_suffix = if attendee.optional { " (optional)" } else { "" };
                                println!("  {} {} <{}>{}", status_icon, name, attendee.email, optional_suffix);
                            }
                        }
                        // Show description last
                        if let Some(desc) = &event.description {
                            println!("\n{}", desc);
                        }
                    } else {
                        #[derive(Serialize)]
                        struct EventDetail {
                            id: String,
                            summary: String,
                            start: String,
                            end: String,
                            #[serde(skip_serializing_if = "Option::is_none")]
                            location: Option<String>,
                            #[serde(skip_serializing_if = "Option::is_none")]
                            description: Option<String>,
                            #[serde(skip_serializing_if = "Option::is_none")]
                            organizer: Option<AttendeeDetail>,
                            attendees: Vec<AttendeeDetail>,
                            account_id: String,
                            calendar_id: String,
                        }
                        let detail = EventDetail {
                            id: event.id.clone(),
                            summary: event.summary.clone(),
                            start: format_event_time(&event.start),
                            end: format_event_time(&event.end),
                            location: event.location.clone(),
                            description: event.description.clone(),
                            organizer: event.organizer.as_ref().map(|o| AttendeeDetail {
                                email: o.email.clone(),
                                name: o.name.clone(),
                                response_status: o.response_status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                                optional: o.optional,
                            }),
                            attendees: event.attendees.iter().map(|a| AttendeeDetail {
                                email: a.email.clone(),
                                name: a.name.clone(),
                                response_status: a.response_status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                                optional: a.optional,
                            }).collect(),
                            account_id: event.account_id.clone(),
                            calendar_id: event.calendar_id.clone(),
                        };
                        println!("{}", serde_json::to_string_pretty(&detail)?);
                    }
                }
                None => {
                    if human {
                        println!("Event not found: {}", id);
                    } else {
                        println!("{{\"error\": \"Event not found\"}}");
                    }
                }
            }
        }

        CalendarCommands::Events { from, to, account, limit, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;

            // Default to today if --from not specified
            let from_date = match &from {
                Some(d) => d.clone(),
                None => chrono::Utc::now().format("%Y-%m-%d").to_string(),
            };

            // Default to 7 days after from if --to not specified
            let to_date = match &to {
                Some(d) => d.clone(),
                None => {
                    let from_parsed = chrono::NaiveDate::parse_from_str(&from_date, "%Y-%m-%d")
                        .unwrap_or_else(|_| chrono::Utc::now().date_naive());
                    (from_parsed + chrono::Duration::days(7)).format("%Y-%m-%d").to_string()
                }
            };

            let accounts_ref = account.as_ref().map(|v| v.iter().map(|s| s.clone()).collect::<Vec<_>>());
            let events = db.list_events_in_range(
                accounts_ref.as_deref(),
                &from_date,
                &to_date,
                limit.min(200),
            ).await?;

            if human {
                if events.is_empty() {
                    println!("No events found from {} to {}.", from_date, to_date);
                } else {
                    println!("\n📅 Events from {} to {} ({} events)\n", from_date, to_date, events.len());
                    let mut current_date = String::new();
                    for event in &events {
                        let event_date = match &event.start {
                            EventTime::DateTime(dt) => dt.format("%Y-%m-%d").to_string(),
                            EventTime::Date(d) => d.to_string(),
                        };
                        if event_date != current_date {
                            current_date = event_date.clone();
                            // Parse and format as weekday
                            if let Ok(d) = chrono::NaiveDate::parse_from_str(&current_date, "%Y-%m-%d") {
                                println!("━━ {} ━━", d.format("%A, %B %e, %Y"));
                            } else {
                                println!("━━ {} ━━", current_date);
                            }
                        }
                        let time_str = match &event.start {
                            EventTime::DateTime(dt) => dt.format("%l:%M %p").to_string(),
                            EventTime::Date(_) => "All day".to_string(),
                        };
                        let duration = match (&event.start, &event.end) {
                            (EventTime::DateTime(s), EventTime::DateTime(e)) => {
                                let mins = (*e - *s).num_minutes();
                                if mins >= 60 {
                                    format!(" ({}h{}m)", mins / 60, mins % 60)
                                } else {
                                    format!(" ({}m)", mins)
                                }
                            }
                            _ => String::new(),
                        };
                        println!("  {} {}{}", time_str.trim(), event.summary, duration);
                        if let Some(loc) = &event.location {
                            if !loc.is_empty() {
                                println!("          📍 {}", loc);
                            }
                        }
                        // Show organizer info if it's someone else's event
                        if let Some(org) = &event.organizer {
                            let org_name = org.name.as_deref().unwrap_or(&org.email);
                            // Check if organizer is different from the account
                            if org.email != event.account_id {
                                println!("          👤 Invited by: {}", org_name);
                            }
                        }
                    }
                    println!();
                }
            } else {
                #[derive(Serialize)]
                struct EventResult {
                    id: String,
                    summary: String,
                    start: String,
                    end: String,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    location: Option<String>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    organizer: Option<AttendeeDetail>,
                    attendees: Vec<AttendeeDetail>,
                    account_id: String,
                    calendar_id: String,
                }
                let results: Vec<EventResult> = events
                    .iter()
                    .map(|e| EventResult {
                        id: e.id.clone(),
                        summary: e.summary.clone(),
                        start: format_event_time(&e.start),
                        end: format_event_time(&e.end),
                        location: e.location.clone(),
                        organizer: e.organizer.as_ref().map(|o| AttendeeDetail {
                            email: o.email.clone(),
                            name: o.name.clone(),
                            response_status: o.response_status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                            optional: o.optional,
                        }),
                        attendees: e.attendees.iter().map(|a| AttendeeDetail {
                            email: a.email.clone(),
                            name: a.name.clone(),
                            response_status: a.response_status.as_ref().map(|s| format!("{:?}", s).to_lowercase()),
                            optional: a.optional,
                        }).collect(),
                        account_id: e.account_id.clone(),
                        calendar_id: e.calendar_id.clone(),
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&results)?);
            }
        }

        CalendarCommands::Create { account, summary, start, end, description, location, attendees, calendar, human } => {
            let human = human || global_human;
            calendar_create(&account, &summary, &start, &end, description.as_deref(), location.as_deref(), attendees, &calendar, human).await?;
        }
    }

    Ok(())
}

fn format_event_time_human(et: &EventTime) -> String {
    match et {
        EventTime::DateTime(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        EventTime::Date(d) => d.to_string(),
    }
}

// ============================================================================
// Account Command Handlers
// ============================================================================

async fn handle_account_command(command: AccountCommands, global_human: bool) -> Result<()> {
    match command {
        AccountCommands::List { human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;
            let accounts = db.list_accounts().await?;

            if human {
                if accounts.is_empty() {
                    println!("No accounts configured.");
                    println!("\nAdd an account with:");
                    println!("  groundeffect-daemon add-account");
                } else {
                    println!("\n📧 Accounts:\n");
                    for account in &accounts {
                        let status_icon = match account.status {
                            AccountStatus::Active => "✓",
                            AccountStatus::NeedsReauth => "⚠",
                            AccountStatus::Disabled => "○",
                            AccountStatus::Syncing => "↻",
                        };
                        let alias = account.alias.as_ref().map(|a| format!(" ({})", a)).unwrap_or_default();
                        println!("{} {}{}", status_icon, account.id, alias);
                        println!("  Status: {:?}", account.status);
                        println!("  Display name: {}", account.display_name);
                        println!();
                    }
                }
            } else {
                let json_results: Vec<AccountResult> = accounts
                    .iter()
                    .map(AccountResult::from_account)
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_results)?);
            }
        }

        AccountCommands::Show { account, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;
            let accounts = db.list_accounts().await?;

            let account_id = resolve_account(&accounts, &account);

            match account_id {
                Some(id) => {
                    if let Some(acct) = db.get_account(&id).await? {
                        let email_count = db.count_emails(Some(&id)).await.unwrap_or(0);
                        let event_count = db.count_events(Some(&id)).await.unwrap_or(0);
                        let (att_total, att_downloaded, att_size) = db.get_attachment_stats(&id).await.unwrap_or((0, 0, 0));

                        if human {
                            println!("\n📧 Account: {}", acct.id);
                            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                            if let Some(alias) = &acct.alias {
                                println!("Alias: {}", alias);
                            }
                            println!("Display name: {}", acct.display_name);
                            println!("Status: {:?}", acct.status);
                            println!("Added: {}", acct.added_at.format("%Y-%m-%d"));
                            println!("\n⚙️  Settings:");
                            if let Some(since) = acct.sync_email_since {
                                println!("  Sync emails since: {}", since.format("%Y-%m-%d"));
                            } else {
                                println!("  Sync emails since: (default)");
                            }
                            println!("  Sync attachments: {}", if acct.sync_attachments { "enabled" } else { "disabled" });
                            println!("\n📊 Stats:");
                            println!("  Emails: {}", email_count);
                            println!("  Events: {}", event_count);
                            println!("  Attachments: {}/{} downloaded ({} bytes)", att_downloaded, att_total, att_size);
                            if let Some(last) = acct.last_sync_email {
                                println!("  Last email sync: {}", last.format("%Y-%m-%d %H:%M"));
                            }
                            if let Some(last) = acct.last_sync_calendar {
                                println!("  Last calendar sync: {}", last.format("%Y-%m-%d %H:%M"));
                            }
                        } else {
                            #[derive(Serialize)]
                            struct AccountDetail {
                                email: String,
                                alias: Option<String>,
                                display_name: String,
                                status: String,
                                added_at: String,
                                email_count: u64,
                                event_count: u64,
                                attachments_total: usize,
                                attachments_downloaded: usize,
                                attachments_size_bytes: u64,
                                last_sync_email: Option<String>,
                                last_sync_calendar: Option<String>,
                                sync_email_since: Option<String>,
                                sync_attachments: bool,
                            }
                            let detail = AccountDetail {
                                email: acct.id.clone(),
                                alias: acct.alias.clone(),
                                display_name: acct.display_name.clone(),
                                status: format!("{:?}", acct.status),
                                added_at: acct.added_at.to_rfc3339(),
                                email_count,
                                event_count,
                                attachments_total: att_total,
                                attachments_downloaded: att_downloaded,
                                attachments_size_bytes: att_size,
                                last_sync_email: acct.last_sync_email.map(|d| d.to_rfc3339()),
                                last_sync_calendar: acct.last_sync_calendar.map(|d| d.to_rfc3339()),
                                sync_email_since: acct.sync_email_since.map(|d| d.to_rfc3339()),
                                sync_attachments: acct.sync_attachments,
                            };
                            println!("{}", serde_json::to_string_pretty(&detail)?);
                        }
                    }
                }
                None => {
                    if human {
                        println!("Account not found: {}", account);
                    } else {
                        println!("{{\"error\": \"Account not found\"}}");
                    }
                }
            }
        }

        AccountCommands::Add { years, attachments, alias, human } => {
            let human = human || global_human;
            account_add(years, attachments, alias, human).await?;
        }

        AccountCommands::Delete { account, confirm, human } => {
            let human = human || global_human;
            account_delete(&account, confirm, human).await?;
        }

        AccountCommands::Configure { account, alias, attachments, no_attachments, human } => {
            let human = human || global_human;
            account_configure(&account, alias, attachments, no_attachments, human).await?;
        }
    }

    Ok(())
}

// ============================================================================
// Sync Command Handlers
// ============================================================================

async fn handle_sync_command(command: SyncCommands, global_human: bool) -> Result<()> {
    match command {
        SyncCommands::Status { account, human } => {
            let human = human || global_human;
            let config = Config::load().unwrap_or_default();
            let db = Database::open(config.lancedb_dir()).await?;
            let accounts = db.list_accounts().await?;

            let target_accounts: Vec<&Account> = if let Some(acct) = &account {
                accounts.iter().filter(|a| a.id == *acct || a.alias.as_ref() == Some(acct)).collect()
            } else {
                accounts.iter().collect()
            };

            if target_accounts.is_empty() {
                if human {
                    println!("No accounts found.");
                } else {
                    println!("[]");
                }
                return Ok(());
            }

            // Check daemon status
            let daemon_running = check_daemon_running();

            if human {
                println!("\n📊 GroundEffect Sync Status\n");
                if daemon_running {
                    println!("Daemon: ✓ running");
                } else {
                    println!("Daemon: ✗ not running");
                }
                println!();
            }

            let mut statuses = Vec::new();

            for account in target_accounts {
                let email_count = db.count_emails(Some(&account.id)).await.unwrap_or(0);
                let event_count = db.count_events(Some(&account.id)).await.unwrap_or(0);
                let (oldest_email, newest_email) = db.get_email_sync_boundaries(&account.id).await.unwrap_or((None, None));
                let (oldest_event, newest_event) = db.get_event_sync_boundaries(&account.id).await.unwrap_or((None, None));
                let (att_total, att_downloaded, att_size) = db.get_attachment_stats(&account.id).await.unwrap_or((0, 0, 0));

                // Calculate remaining emails if we have an estimate
                let emails_remaining = account.estimated_total_emails.map(|total| {
                    total.saturating_sub(email_count)
                });

                let status = SyncStatus {
                    account: account.id.clone(),
                    status: format!("{:?}", account.status),
                    email_count,
                    event_count,
                    oldest_email: oldest_email.map(|d| d.format("%Y-%m-%d").to_string()),
                    newest_email: newest_email.map(|d| d.format("%Y-%m-%d").to_string()),
                    oldest_event: oldest_event.map(|d| d.format("%Y-%m-%d").to_string()),
                    newest_event: newest_event.map(|d| d.format("%Y-%m-%d").to_string()),
                    last_email_sync: account.last_sync_email.map(|d| d.to_rfc3339()),
                    last_calendar_sync: account.last_sync_calendar.map(|d| d.to_rfc3339()),
                    attachments_total: att_total,
                    attachments_downloaded: att_downloaded,
                    attachments_size_bytes: att_size,
                    sync_email_since: account.sync_email_since.map(|d| d.to_rfc3339()),
                    sync_attachments: account.sync_attachments,
                    estimated_total_emails: account.estimated_total_emails,
                    emails_remaining,
                };

                if human {
                    let status_icon = match account.status {
                        AccountStatus::Active => "✓",
                        AccountStatus::NeedsReauth => "⚠",
                        AccountStatus::Disabled => "○",
                        AccountStatus::Syncing => "↻",
                    };
                    let alias = account.alias.as_ref().map(|a| format!(" ({})", a)).unwrap_or_default();
                    println!("{}  {}{}", status_icon, account.id, alias);
                    println!("   Status: {:?}", account.status);
                    if let Some(since) = account.sync_email_since {
                        println!("   ⚙️  Sync since: {}", since.format("%Y-%m-%d"));
                    }
                    // Show email count with total and remaining if available
                    if let Some(total) = account.estimated_total_emails {
                        let remaining = total.saturating_sub(email_count);
                        if remaining > 0 {
                            println!("   📨 Emails: {} / {} ({} remaining)", email_count, total, remaining);
                        } else {
                            println!("   📨 Emails: {} (sync complete)", email_count);
                        }
                    } else {
                        println!("   📨 Emails: {}", email_count);
                    }
                    if let Some(oldest) = &status.oldest_email {
                        println!("      Oldest: {}", oldest);
                    }
                    if let Some(newest) = &status.newest_email {
                        println!("      Newest: {}", newest);
                    }
                    if let Some(last) = account.last_sync_email {
                        println!("      Last sync: {}", format_relative_time(last));
                    }
                    println!("   📅 Events: {}", event_count);
                    if let Some(oldest) = &status.oldest_event {
                        println!("      Oldest: {}", oldest);
                    }
                    if let Some(newest) = &status.newest_event {
                        println!("      Newest: {}", newest);
                    }
                    if let Some(last) = account.last_sync_calendar {
                        println!("      Last sync: {}", format_relative_time(last));
                    }
                    println!("   📎 Attachments: {}/{} downloaded ({})", att_downloaded, att_total, format_bytes(att_size));
                    if account.sync_attachments {
                        println!("      Auto-download: enabled");
                    }
                    println!();
                }

                statuses.push(status);
            }

            if !human {
                println!("{}", serde_json::to_string_pretty(&statuses)?);
            }
        }

        SyncCommands::Reset { account, data_type, confirm, human } => {
            let human = human || global_human;
            sync_reset(&account, &data_type, confirm, human).await?;
        }

        SyncCommands::Extend { account, target_date, human } => {
            let human = human || global_human;
            sync_extend(&account, &target_date, human).await?;
        }

        SyncCommands::ResumeFrom { account, target_date, human } => {
            let human = human || global_human;
            sync_resume_from(&account, &target_date, human).await?;
        }

        SyncCommands::DownloadAttachments { account, human } => {
            let human = human || global_human;
            sync_download_attachments(&account, human).await?;
        }
    }

    Ok(())
}

// ============================================================================
// Daemon Command Handlers
// ============================================================================

async fn handle_daemon_command(command: DaemonCommands, global_human: bool) -> Result<()> {
    match command {
        DaemonCommands::Status { human } => {
            let human = human || global_human;
            let running = check_daemon_running();
            let pid = get_daemon_pid();

            if human {
                if running {
                    println!("Daemon: ✓ running (PID: {})", pid.unwrap_or(0));
                } else {
                    println!("Daemon: ✗ not running");
                }
            } else {
                let status = DaemonStatus { running, pid };
                println!("{}", serde_json::to_string_pretty(&status)?);
            }
        }

        DaemonCommands::Start { logging, human } => {
            let human = human || global_human;
            if check_daemon_running() {
                if human {
                    println!("Daemon is already running.");
                } else {
                    println!("{{\"status\": \"already_running\"}}");
                }
                return Ok(());
            }

            let plist_path = dirs::home_dir()
                .unwrap_or_default()
                .join("Library/LaunchAgents/com.groundeffect.daemon.plist");

            if plist_path.exists() {
                let output = std::process::Command::new("launchctl")
                    .args(["load", "-w", plist_path.to_str().unwrap()])
                    .output()?;

                if human {
                    if output.status.success() {
                        println!("✓ Daemon started via launchd");
                    } else {
                        println!("Failed to start daemon: {}", String::from_utf8_lossy(&output.stderr));
                    }
                } else {
                    if output.status.success() {
                        println!("{{\"status\": \"started\", \"method\": \"launchd\"}}");
                    } else {
                        println!("{{\"status\": \"error\", \"message\": \"{}\" }}", String::from_utf8_lossy(&output.stderr).replace('"', "\\\""));
                    }
                }
            } else {
                let mut cmd = std::process::Command::new("groundeffect-daemon");
                if logging {
                    cmd.arg("--log");
                }
                cmd.spawn()?;

                if human {
                    println!("✓ Daemon started directly");
                } else {
                    println!("{{\"status\": \"started\", \"method\": \"direct\"}}");
                }
            }
        }

        DaemonCommands::Stop { human } => {
            let human = human || global_human;
            if !check_daemon_running() {
                if human {
                    println!("Daemon is not running.");
                } else {
                    println!("{{\"status\": \"not_running\"}}");
                }
                return Ok(());
            }

            let plist_path = dirs::home_dir()
                .unwrap_or_default()
                .join("Library/LaunchAgents/com.groundeffect.daemon.plist");

            if plist_path.exists() {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap()])
                    .output();
            }

            let _ = std::process::Command::new("pkill")
                .args(["-f", "groundeffect-daemon"])
                .output();

            if human {
                println!("✓ Daemon stopped");
            } else {
                println!("{{\"status\": \"stopped\"}}");
            }
        }

        DaemonCommands::Restart { logging, human } => {
            let human = human || global_human;

            let _ = std::process::Command::new("pkill")
                .args(["-f", "groundeffect-daemon"])
                .output();

            std::thread::sleep(std::time::Duration::from_millis(500));

            let plist_path = dirs::home_dir()
                .unwrap_or_default()
                .join("Library/LaunchAgents/com.groundeffect.daemon.plist");

            if plist_path.exists() {
                let _ = std::process::Command::new("launchctl")
                    .args(["load", "-w", plist_path.to_str().unwrap()])
                    .output();

                if human {
                    println!("✓ Daemon restarted via launchd");
                } else {
                    println!("{{\"status\": \"restarted\", \"method\": \"launchd\"}}");
                }
            } else {
                let mut cmd = std::process::Command::new("groundeffect-daemon");
                if logging {
                    cmd.arg("--log");
                }
                cmd.spawn()?;

                if human {
                    println!("✓ Daemon restarted");
                } else {
                    println!("{{\"status\": \"restarted\", \"method\": \"direct\"}}");
                }
            }
        }

        DaemonCommands::Install { logging, human } => {
            let human = human || global_human;
            daemon_install(logging, human)?;
        }

        DaemonCommands::Uninstall { human } => {
            let human = human || global_human;
            daemon_uninstall(human)?;
        }
    }

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

fn check_daemon_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", "groundeffect-daemon"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn get_daemon_pid() -> Option<u32> {
    std::process::Command::new("pgrep")
        .args(["-f", "groundeffect-daemon"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .and_then(|s| s.trim().parse().ok())
            } else {
                None
            }
        })
}

/// Restart the daemon. Returns the method used ("launchd" or "direct") or None if not running.
fn restart_daemon() -> Option<&'static str> {
    // Check if daemon is running first
    if !check_daemon_running() {
        return None;
    }

    // Kill existing daemon
    let _ = std::process::Command::new("pkill")
        .args(["-f", "groundeffect-daemon"])
        .output();

    std::thread::sleep(std::time::Duration::from_millis(500));

    let plist_path = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/LaunchAgents/com.groundeffect.daemon.plist");

    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["load", "-w", plist_path.to_str().unwrap()])
            .output();
        Some("launchd")
    } else {
        let _ = std::process::Command::new("groundeffect-daemon")
            .spawn();
        Some("direct")
    }
}

fn resolve_account(accounts: &[Account], query: &str) -> Option<String> {
    accounts
        .iter()
        .find(|a| a.id == query || a.alias.as_ref() == Some(&query.to_string()))
        .map(|a| a.id.clone())
}

/// Parse a date string in the user's timezone and convert to UTC.
///
/// If timezone parsing fails, falls back to UTC.
fn parse_date(date_str: &Option<String>, timezone: &str) -> Option<DateTime<Utc>> {
    date_str.as_ref().and_then(|s| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| {
            let naive_dt = d.and_hms_opt(0, 0, 0).unwrap();

            // Try to parse the timezone, fallback to UTC
            if let Ok(tz) = timezone.parse::<Tz>() {
                // Convert from user's timezone to UTC
                tz.from_local_datetime(&naive_dt)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|| naive_dt.and_utc())
            } else {
                // Invalid timezone string, treat as UTC
                naive_dt.and_utc()
            }
        })
    })
}

fn format_relative_time(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    if duration.num_seconds() < 60 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        let mins = duration.num_minutes();
        format!("{} minute{} ago", mins, if mins == 1 { "" } else { "s" })
    } else if duration.num_hours() < 24 {
        let hours = duration.num_hours();
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else if duration.num_days() < 7 {
        let days = duration.num_days();
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    } else {
        dt.format("%Y-%m-%d %H:%M").to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

// ============================================================================
// Daemon Install/Uninstall Functions
// ============================================================================

fn daemon_install(logging: Option<bool>, human: bool) -> Result<()> {
    let plist_path = DaemonConfig::launchd_plist_path();

    // Check if already installed
    if plist_path.exists() {
        if human {
            println!("Launchd agent already installed at {:?}", plist_path);
            println!("To reinstall, run: groundeffect daemon uninstall && groundeffect daemon install");
        } else {
            println!("{{\"status\": \"already_installed\", \"plist_path\": \"{}\"}}", plist_path.display());
        }
        return Ok(());
    }

    // Load existing config or create defaults
    let mut daemon_config = DaemonConfig::load().unwrap_or_default();

    // Only override logging if explicitly specified
    if let Some(log_enabled) = logging {
        daemon_config.logging_enabled = log_enabled;
    }
    daemon_config.save()?;

    // Find daemon binary
    let daemon_path = find_daemon_binary()?;

    // Create LaunchAgents directory
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create log directory
    let log_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".local")
        .join("share")
        .join("groundeffect")
        .join("logs");
    std::fs::create_dir_all(&log_dir)?;

    // Generate plist content
    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.groundeffect.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>-c</string>
        <string>source ~/.secrets 2>/dev/null; exec {daemon_path}{logging_flag}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
    <key>SoftResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>65536</integer>
    </dict>
    <key>EnvironmentVariables</key>
    <dict>
        <key>GROUNDEFFECT_EMAIL_POLL_INTERVAL</key>
        <string>{email_interval}</string>
        <key>GROUNDEFFECT_CALENDAR_POLL_INTERVAL</key>
        <string>{calendar_interval}</string>
        <key>GROUNDEFFECT_MAX_CONCURRENT_FETCHES</key>
        <string>{max_fetches}</string>
    </dict>
</dict>
</plist>"#,
        daemon_path = daemon_path.display(),
        logging_flag = if daemon_config.logging_enabled { " --log" } else { "" },
        stdout = log_dir.join("stdout.log").display(),
        stderr = log_dir.join("stderr.log").display(),
        email_interval = daemon_config.email_poll_interval_secs,
        calendar_interval = daemon_config.calendar_poll_interval_secs,
        max_fetches = daemon_config.max_concurrent_fetches,
    );

    std::fs::write(&plist_path, plist_content)?;

    // Load the launchd agent
    let output = std::process::Command::new("launchctl")
        .args(["load", "-w", plist_path.to_str().unwrap()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("service already loaded") {
            if human {
                println!("Failed to load launchd agent: {}", stderr);
            } else {
                println!("{{\"status\": \"error\", \"message\": \"{}\"}}", stderr.replace('"', "\\\""));
            }
            return Ok(());
        }
    }

    if human {
        println!("✓ Launchd agent installed");
        println!("  Plist: {:?}", plist_path);
        println!("  Config: {:?}", DaemonConfig::config_path());
        println!("  Logs: {:?}", log_dir);
        println!("\nThe daemon will start automatically at login.");
        println!("To customize settings: groundeffect config settings");
    } else {
        println!("{{\"status\": \"installed\", \"plist_path\": \"{}\", \"config_path\": \"{}\"}}",
            plist_path.display(),
            DaemonConfig::config_path().display());
    }

    Ok(())
}

fn daemon_uninstall(human: bool) -> Result<()> {
    let plist_path = DaemonConfig::launchd_plist_path();

    if !plist_path.exists() {
        if human {
            println!("Launchd agent is not installed.");
        } else {
            println!("{{\"status\": \"not_installed\"}}");
        }
        return Ok(());
    }

    // Unload the agent
    let _ = std::process::Command::new("launchctl")
        .args(["unload", "-w", plist_path.to_str().unwrap()])
        .output();

    // Remove the plist file
    std::fs::remove_file(&plist_path)?;

    if human {
        println!("✓ Launchd agent uninstalled");
        println!("The daemon will no longer start automatically at login.");
        println!("\nNote: Synced data and configuration files are preserved.");
    } else {
        println!("{{\"status\": \"uninstalled\"}}");
    }

    Ok(())
}

fn find_daemon_binary() -> Result<std::path::PathBuf> {
    // Check common installation paths
    let home = dirs::home_dir().unwrap_or_default();
    let paths = [
        // Homebrew on Apple Silicon
        std::path::PathBuf::from("/opt/homebrew/bin/groundeffect-daemon"),
        // Homebrew on Intel
        std::path::PathBuf::from("/usr/local/bin/groundeffect-daemon"),
        // Cargo install
        home.join(".cargo/bin/groundeffect-daemon"),
    ];

    for path in &paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    // Use which to find it
    let output = std::process::Command::new("which")
        .arg("groundeffect-daemon")
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(std::path::PathBuf::from(path));
            }
        }
    }

    anyhow::bail!("Could not find groundeffect-daemon binary. Make sure it's installed and in your PATH.")
}

// ============================================================================
// Account Add/Delete/Configure Functions
// ============================================================================

async fn account_add(years: Option<String>, attachments: bool, alias: Option<String>, human: bool) -> Result<()> {
    use dialoguer::{Input, Select, Confirm};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    // Load config and initialize token provider early
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;

    // Check for OAuth credentials
    let client_id = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_ID");
    let client_secret = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_SECRET");

    if client_id.is_err() || client_secret.is_err() {
        if human {
            println!("\n❌ OAuth credentials not configured!\n");
            println!("Please set the following environment variables:");
            println!("  export GROUNDEFFECT_GOOGLE_CLIENT_ID=\"your-client-id\"");
            println!("  export GROUNDEFFECT_GOOGLE_CLIENT_SECRET=\"your-client-secret\"");
            println!("\nYou can get these from the Google Cloud Console:");
            println!("  https://console.cloud.google.com/apis/credentials");
            println!("\nMake sure to:");
            println!("  1. Create an OAuth 2.0 Client ID (Desktop app type)");
            println!("  2. Add http://localhost:8085/oauth/callback as a redirect URI");
            println!("  3. Enable Gmail API and Google Calendar API\n");
        } else {
            println!("{{\"success\": false, \"error\": \"OAuth credentials not configured\"}}");
        }
        return Ok(());
    }

    // Interactive prompts if not provided via args
    let years_to_sync = match years {
        Some(y) => y,
        None => {
            if human {
                let options = &["1 year (recommended)", "2 years", "5 years", "10 years", "All email history"];
                let selection = Select::new()
                    .with_prompt("How many years of email history to sync?")
                    .items(options)
                    .default(0)
                    .interact()?;
                match selection {
                    0 => "1".to_string(),
                    1 => "2".to_string(),
                    2 => "5".to_string(),
                    3 => "10".to_string(),
                    _ => "all".to_string(),
                }
            } else {
                "1".to_string() // Default for non-interactive
            }
        }
    };

    let sync_attachments = if !attachments && human {
        Confirm::new()
            .with_prompt("Enable automatic attachment downloading?")
            .default(false)
            .interact()?
    } else {
        attachments
    };

    let account_alias = match alias {
        Some(a) => if a.is_empty() { None } else { Some(a) },
        None => {
            if human {
                let input: String = Input::new()
                    .with_prompt("Alias for this account (optional, press Enter to skip)")
                    .allow_empty(true)
                    .interact_text()?;
                if input.is_empty() { None } else { Some(input) }
            } else {
                None
            }
        }
    };

    // Parse years_to_sync
    let sync_since = if years_to_sync.eq_ignore_ascii_case("all") {
        None
    } else {
        let years: u32 = years_to_sync.parse().unwrap_or(1);
        let years = years.clamp(1, 20);
        Some(Utc::now() - chrono::Duration::days(years as i64 * 365))
    };

    if human {
        println!("\n🔐 Opening browser for Google authentication...\n");
    }

    let oauth = OAuthManager::new(token_provider.clone());
    let state = format!("groundeffect_{}", uuid::Uuid::new_v4());
    let auth_url = oauth.authorization_url(&state);

    if human {
        println!("If the browser doesn't open, visit this URL manually:");
        println!("{}\n", auth_url);
    }

    // Open browser
    if let Err(e) = open::that(&auth_url) {
        if human {
            println!("Failed to open browser: {}", e);
        }
    }

    // Start local HTTP server to receive callback
    let listener = TcpListener::bind("127.0.0.1:8085").await?;
    if human {
        println!("⏳ Waiting for authentication callback on http://localhost:8085 ...\n");
    }

    // Accept one connection with timeout
    let callback_result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        async {
            let (mut socket, _) = listener.accept().await?;
            let mut reader = BufReader::new(&mut socket);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).await?;

            // Parse callback
            let (code, received_state) = parse_oauth_callback(&request_line)?;

            if received_state != state {
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<h1>Error: Invalid state</h1>";
                socket.write_all(response.as_bytes()).await?;
                anyhow::bail!("OAuth state mismatch - possible CSRF attack");
            }

            // Send success response
            let success_html = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
                <!DOCTYPE html><html><body style='font-family: sans-serif; padding: 40px; text-align: center;'>\
                <h1>Authentication Successful!</h1><p>You can close this window.</p></body></html>";
            socket.write_all(success_html.as_bytes()).await?;

            Ok::<String, anyhow::Error>(code)
        }
    ).await;

    let code = match callback_result {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => {
            if human {
                println!("❌ OAuth error: {}", e);
            } else {
                println!("{{\"success\": false, \"error\": \"{}\"}}", e);
            }
            return Ok(());
        }
        Err(_) => {
            if human {
                println!("❌ OAuth timeout: no callback received within 5 minutes");
            } else {
                println!("{{\"success\": false, \"error\": \"OAuth timeout\"}}");
            }
            return Ok(());
        }
    };

    if human {
        println!("✅ Received authorization code, exchanging for tokens...\n");
    }

    // Exchange code for tokens
    let (tokens, user_info) = oauth.exchange_code(&code).await?;

    // Store tokens
    token_provider.store_tokens(&user_info.email, &tokens).await?;

    // Open database and create/update account
    std::fs::create_dir_all(config.lancedb_dir())?;
    let db = Database::open(config.lancedb_dir()).await?;

    let is_existing = db.get_account(&user_info.email).await?.is_some();

    if let Some(existing) = db.get_account(&user_info.email).await? {
        let mut updated = existing;
        updated.status = AccountStatus::Active;
        updated.alias = account_alias.clone().or(updated.alias);
        updated.sync_email_since = sync_since;
        updated.sync_attachments = sync_attachments;
        db.upsert_account(&updated).await?;
    } else {
        let account = Account {
            id: user_info.email.clone(),
            alias: account_alias.clone(),
            display_name: user_info.name.unwrap_or_else(|| user_info.email.clone()),
            added_at: Utc::now(),
            last_sync_email: None,
            last_sync_calendar: None,
            status: AccountStatus::Active,
            sync_email_since: sync_since,
            oldest_email_synced: None,
            oldest_event_synced: None,
            sync_attachments,
            estimated_total_emails: None,
        };
        db.upsert_account(&account).await?;
    }

    if human {
        if is_existing {
            println!("🎉 Re-authenticated account: {}", user_info.email);
        } else {
            println!("🎉 Successfully added account: {}", user_info.email);
        }
        if let Some(a) = &account_alias {
            println!("   Alias: {}", a);
        }
        println!("   Sync {} of email history", years_to_sync);
        println!("   Attachments: {}", if sync_attachments { "enabled" } else { "disabled" });
        println!("\nThe daemon will start syncing automatically if running.");
    } else {
        println!("{{\"success\": true, \"account\": {{\"id\": \"{}\", \"alias\": {}, \"years_to_sync\": \"{}\", \"sync_attachments\": {}}}}}",
            user_info.email,
            account_alias.as_ref().map(|a| format!("\"{}\"", a)).unwrap_or("null".to_string()),
            years_to_sync,
            sync_attachments);
    }

    Ok(())
}

fn parse_oauth_callback(request_line: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("Invalid HTTP request");
    }

    let path = parts[1];
    if !path.starts_with("/oauth/callback") {
        anyhow::bail!("Unexpected callback path: {}", path);
    }

    let query_start = path.find('?').ok_or_else(|| anyhow::anyhow!("No query string"))?;
    let query = &path[query_start + 1..];

    let mut code = None;
    let mut state = None;

    for param in query.split('&') {
        let mut kv = param.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");
        match key {
            "code" => code = Some(urlencoding::decode(value).unwrap_or_default().into_owned()),
            "state" => state = Some(urlencoding::decode(value).unwrap_or_default().into_owned()),
            _ => {}
        }
    }

    let code = code.ok_or_else(|| anyhow::anyhow!("No authorization code in callback"))?;
    let state = state.ok_or_else(|| anyhow::anyhow!("No state in callback"))?;

    Ok((code, state))
}

async fn account_delete(account: &str, confirm: bool, human: bool) -> Result<()> {
    if !confirm {
        if human {
            println!("❌ Must pass --confirm flag to delete an account.");
            println!("\nThis will permanently delete all synced emails and events for this account.");
            println!("Example: groundeffect account delete {} --confirm", account);
        } else {
            println!("{{\"success\": false, \"error\": \"Must pass --confirm to delete\"}}");
        }
        return Ok(());
    }

    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let email = accounts.iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone());

    match email {
        Some(email) => {
            // Delete synced data
            let (email_count, event_count) = db.clear_account_sync_data(&email).await?;

            // Delete account
            db.delete_account(&email).await?;

            // Delete tokens
            if let Err(e) = token_provider.delete_tokens(&email).await {
                if human {
                    println!("Warning: Failed to delete tokens: {}", e);
                }
            }

            if human {
                println!("✅ Account deleted: {}", email);
                println!("   {} emails removed", email_count);
                println!("   {} events removed", event_count);
            } else {
                println!("{{\"success\": true, \"deleted\": {{\"account\": \"{}\", \"emails\": {}, \"events\": {}}}}}",
                    email, email_count, event_count);
            }
        }
        None => {
            if human {
                println!("❌ Account not found: {}", account);
            } else {
                println!("{{\"success\": false, \"error\": \"Account not found\"}}");
            }
        }
    }

    Ok(())
}

async fn account_configure(account: &str, alias: Option<String>, attachments: bool, no_attachments: bool, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let email = accounts.iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone());

    match email {
        Some(email) => {
            let mut acct = db.get_account(&email).await?.unwrap();
            let mut changes = vec![];

            // Update alias
            if let Some(new_alias) = alias {
                if new_alias.is_empty() {
                    if acct.alias.is_some() {
                        acct.alias = None;
                        changes.push("alias removed".to_string());
                    }
                } else {
                    acct.alias = Some(new_alias.clone());
                    changes.push(format!("alias set to '{}'", new_alias));
                }
            }

            // Update attachments
            if attachments && !no_attachments {
                if !acct.sync_attachments {
                    acct.sync_attachments = true;
                    changes.push("sync_attachments enabled".to_string());
                }
            } else if no_attachments && !attachments {
                if acct.sync_attachments {
                    acct.sync_attachments = false;
                    changes.push("sync_attachments disabled".to_string());
                }
            }

            if changes.is_empty() {
                if human {
                    println!("No changes specified.");
                    println!("\nCurrent settings:");
                    println!("  Alias: {}", acct.alias.as_deref().unwrap_or("(none)"));
                    println!("  Sync attachments: {}", acct.sync_attachments);
                } else {
                    println!("{{\"success\": true, \"changes\": [], \"account\": {{\"id\": \"{}\", \"alias\": {}, \"sync_attachments\": {}}}}}",
                        acct.id,
                        acct.alias.as_ref().map(|a| format!("\"{}\"", a)).unwrap_or("null".to_string()),
                        acct.sync_attachments);
                }
            } else {
                db.upsert_account(&acct).await?;

                if human {
                    println!("✅ Account configured: {}", email);
                    for change in &changes {
                        println!("   - {}", change);
                    }
                    if changes.iter().any(|c| c.contains("sync_attachments")) {
                        println!("\nRestart the daemon for attachment changes to take effect.");
                    }
                } else {
                    println!("{{\"success\": true, \"changes\": {:?}, \"account\": {{\"id\": \"{}\", \"alias\": {}, \"sync_attachments\": {}}}}}",
                        changes,
                        acct.id,
                        acct.alias.as_ref().map(|a| format!("\"{}\"", a)).unwrap_or("null".to_string()),
                        acct.sync_attachments);
                }
            }
        }
        None => {
            if human {
                println!("❌ Account not found: {}", account);
            } else {
                println!("{{\"success\": false, \"error\": \"Account not found\"}}");
            }
        }
    }

    Ok(())
}

// ============================================================================
// Config Command Handlers
// ============================================================================

async fn handle_config_command(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::AddPermissions => config_add_permissions().await,
        ConfigCommands::RemovePermissions => config_remove_permissions().await,
        ConfigCommands::Settings { logging, email_interval, calendar_interval, max_fetches, timezone, human } => {
            config_settings(logging, email_interval, calendar_interval, max_fetches, timezone, human).await
        }
    }
}

async fn config_add_permissions() -> Result<()> {
    use std::fs;
    use std::path::PathBuf;

    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    let home_path = PathBuf::from(&home);
    let settings_path = home_path.join(".claude").join("settings.json");

    let permission = "Bash(groundeffect:*)";

    // Read existing settings or create new structure
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        serde_json::json!({})
    };

    // Ensure permissions.allow array exists
    if settings.get("permissions").is_none() {
        settings["permissions"] = serde_json::json!({});
    }
    if settings["permissions"].get("allow").is_none() {
        settings["permissions"]["allow"] = serde_json::json!([]);
    }

    let allow_list = settings["permissions"]["allow"].as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("permissions.allow is not an array"))?;

    if allow_list.iter().any(|v| v.as_str() == Some(permission)) {
        println!("✓ groundeffect already in Claude Code allowlist");
        println!("  {}", settings_path.display());
    } else {
        allow_list.push(serde_json::json!(permission));

        let content = serde_json::to_string_pretty(&settings)?;
        fs::write(&settings_path, content)?;

        println!("✓ Added groundeffect to Claude Code allowlist");
        println!("  {}", settings_path.display());
    }

    // Install skill files to ~/.claude/skills/groundeffect/
    // Look for skill source in common locations
    // Note: This may fail during homebrew post_install due to sandbox restrictions,
    // so we catch and report errors but don't fail the overall command
    let skill_dest = home_path.join(".claude").join("skills").join("groundeffect");
    let skill_sources = [
        // Homebrew share directory
        PathBuf::from("/opt/homebrew/share/groundeffect/skill"),
        // Intel Mac homebrew
        PathBuf::from("/usr/local/share/groundeffect/skill"),
    ];

    if let Some(skill_source) = skill_sources.iter().find(|p| p.exists()) {
        // Copy all files recursively
        fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let src_path = entry.path();
                let dst_path = dst.join(entry.file_name());

                if src_path.is_dir() {
                    std::fs::create_dir_all(&dst_path)?;
                    copy_dir_recursive(&src_path, &dst_path)?;
                } else {
                    std::fs::copy(&src_path, &dst_path)?;
                }
            }
            Ok(())
        }

        let install_result = (|| -> std::io::Result<()> {
            if skill_dest.exists() {
                fs::remove_dir_all(&skill_dest)?;
            }
            fs::create_dir_all(&skill_dest)?;
            copy_dir_recursive(skill_source, &skill_dest)?;
            Ok(())
        })();

        match install_result {
            Ok(()) => {
                println!("✓ Installed groundeffect skill");
                println!("  {}", skill_dest.display());
            }
            Err(e) => {
                // Don't fail the command, just note that manual installation may be needed
                eprintln!("Note: Could not install skill files ({})", e);
                eprintln!("  Run 'groundeffect config add-permissions' outside of homebrew to install skills");
            }
        }
    }

    Ok(())
}

async fn config_remove_permissions() -> Result<()> {
    use std::fs;
    use std::path::PathBuf;

    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    let settings_path = PathBuf::from(&home).join(".claude").join("settings.json");

    if !settings_path.exists() {
        println!("No Claude Code settings file found");
        println!("  {}", settings_path.display());
        return Ok(());
    }

    let permission = "Bash(groundeffect:*)";

    let content = fs::read_to_string(&settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let removed = if let Some(allow_list) = settings.get_mut("permissions")
        .and_then(|p| p.get_mut("allow"))
        .and_then(|a| a.as_array_mut())
    {
        let initial_len = allow_list.len();
        allow_list.retain(|v| v.as_str() != Some(permission));
        allow_list.len() < initial_len
    } else {
        false
    };

    if removed {
        let content = serde_json::to_string_pretty(&settings)?;
        fs::write(&settings_path, content)?;
        println!("✓ Removed groundeffect from Claude Code allowlist");
        println!("  {}", settings_path.display());
    } else {
        println!("groundeffect not in Claude Code allowlist");
        println!("  {}", settings_path.display());
    }

    Ok(())
}

// ============================================================================
// Email Send/Attachment Functions
// ============================================================================

async fn email_send(
    from: &str,
    to: Vec<String>,
    subject: &str,
    body: &str,
    cc: Option<Vec<String>>,
    bcc: Option<Vec<String>>,
    reply_to: Option<String>,
    force_html: bool,
    save_as_draft: bool,
    confirm: bool,
    human: bool,
) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account and get display name
    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;
    let display_name = &account.display_name;

    let cc_list = cc.unwrap_or_default();
    let bcc_list = bcc.unwrap_or_default();

    // Build reply headers if replying
    let mut in_reply_to = None;
    let mut references = None;
    let mut final_subject = subject.to_string();

    if let Some(reply_id) = &reply_to {
        if let Ok(Some(original)) = db.get_email(reply_id).await {
            in_reply_to = Some(original.message_id.clone());
            references = Some(original.message_id.clone());
            if !final_subject.starts_with("Re:") && !final_subject.starts_with("RE:") {
                final_subject = format!("Re: {}", original.subject);
            }
        }
    }

    // Detect if HTML formatting is needed
    let is_html = force_html || detect_html_content(body);

    // If not confirmed and not saving as draft, return preview
    if !confirm && !save_as_draft {
        if human {
            println!("\n📧 Email Preview (NOT SENT)");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("From: {} <{}>", display_name, from_email);
            println!("To: {}", to.join(", "));
            if !cc_list.is_empty() {
                println!("CC: {}", cc_list.join(", "));
            }
            if !bcc_list.is_empty() {
                println!("BCC: {}", bcc_list.join(", "));
            }
            println!("Subject: {}", final_subject);
            if is_html {
                println!("Format: HTML (auto-detected or forced)");
            }
            if in_reply_to.is_some() {
                println!("(Reply to message)");
            }
            println!("\n{}", body);
            println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("To send: add --confirm | To save as draft: add --save-as-draft");
        } else {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "status": "preview",
                "message": "Add --confirm to send, or --save-as-draft to save as draft",
                "email": {
                    "from": format!("{} <{}>", display_name, from_email),
                    "to": to,
                    "cc": cc_list,
                    "bcc": bcc_list,
                    "subject": final_subject,
                    "body": body,
                    "is_html": is_html,
                    "in_reply_to": in_reply_to,
                    "references": references,
                }
            }))?);
        }
        return Ok(());
    }

    // Build RFC 2822 message with HTML support
    let message = build_email_message(
        display_name,
        from_email,
        &to,
        &cc_list,
        &bcc_list,
        &final_subject,
        body,
        is_html,
        in_reply_to.as_deref(),
        references.as_deref(),
    );

    // Base64url encode the message
    let encoded = URL_SAFE_NO_PAD.encode(message.as_bytes());

    // Get access token
    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    // If saving as draft
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
            if human {
                println!("❌ Failed to create draft: {} - {}", status, error_body);
            } else {
                println!("{{\"status\": \"error\", \"message\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
            }
            return Ok(());
        }

        let result: serde_json::Value = response.json().await?;
        let draft_id = result["id"].as_str().unwrap_or("unknown");
        let message_id = result["message"]["id"].as_str().unwrap_or("unknown");

        if human {
            println!("✅ Draft created successfully!");
            println!("   Draft ID: {}", draft_id);
            println!("   To: {}", to.join(", "));
            println!("   Subject: {}", final_subject);
            println!("\nUse 'groundeffect email draft send --from {} --draft-id {}' to send", from, draft_id);
        } else {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "status": "draft_created",
                "draft_id": draft_id,
                "message_id": message_id,
                "from": format!("{} <{}>", display_name, from_email),
                "to": to,
                "subject": final_subject,
            }))?);
        }
        return Ok(());
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
        if human {
            println!("❌ Failed to send email: {} - {}", status, error_body);
        } else {
            println!("{{\"status\": \"error\", \"message\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let result: serde_json::Value = response.json().await?;
    let message_id = result["id"].as_str().unwrap_or("unknown");

    if human {
        println!("✅ Email sent successfully!");
        println!("   Message ID: {}", message_id);
        println!("   To: {}", to.join(", "));
        println!("   Subject: {}", final_subject);
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "status": "sent",
            "message_id": message_id,
            "from": format!("{} <{}>", display_name, from_email),
            "to": to,
            "subject": final_subject,
        }))?);
    }

    Ok(())
}

async fn email_attachment(
    email_id: &str,
    filename: Option<&str>,
    attachment_id: Option<&str>,
    human: bool,
) -> Result<()> {
    if filename.is_none() && attachment_id.is_none() {
        if human {
            println!("❌ Must provide either --filename or --attachment-id");
        } else {
            println!("{{\"error\": \"Must provide either filename or attachment_id\"}}");
        }
        return Ok(());
    }

    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;

    let email = match db.get_email(email_id).await? {
        Some(e) => e,
        None => {
            if human {
                println!("❌ Email not found: {}", email_id);
            } else {
                println!("{{\"error\": \"Email not found\"}}");
            }
            return Ok(());
        }
    };

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
        });

    let attachment = match attachment {
        Some(a) => a,
        None => {
            let search = attachment_id.or(filename).unwrap_or("unknown");
            if human {
                println!("❌ Attachment not found: {}", search);
                println!("\nAvailable attachments:");
                for a in &email.attachments {
                    println!("  - {} (id: {})", a.filename, a.id);
                }
            } else {
                println!("{{\"error\": \"Attachment not found: {}\"}}", search);
            }
            return Ok(());
        }
    };

    // Check if downloaded
    if !attachment.downloaded {
        if human {
            println!("❌ Attachment not downloaded: {}", attachment.filename);
            println!("\nTo download attachments:");
            println!("  groundeffect sync download-attachments --account <account>");
        } else {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "error": "not_downloaded",
                "message": "Attachment not downloaded. Use 'sync download-attachments' command.",
                "attachment": {
                    "id": attachment.id,
                    "filename": attachment.filename,
                    "mime_type": attachment.mime_type,
                    "size": attachment.size,
                }
            }))?);
        }
        return Ok(());
    }

    let local_path = match &attachment.local_path {
        Some(p) => p,
        None => {
            if human {
                println!("❌ Attachment marked as downloaded but no local path");
            } else {
                println!("{{\"error\": \"Attachment marked as downloaded but no local_path\"}}");
            }
            return Ok(());
        }
    };

    if !local_path.exists() {
        if human {
            println!("❌ Attachment file missing: {:?}", local_path);
        } else {
            println!("{{\"error\": \"Attachment file missing\", \"path\": \"{}\"}}", local_path.display());
        }
        return Ok(());
    }

    // Check if it's a text file
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
        || attachment.filename.ends_with(".toml");

    if human {
        println!("\n📎 Attachment: {}", attachment.filename);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Type: {}", attachment.mime_type);
        println!("Size: {} bytes", attachment.size);
        println!("Path: {:?}", local_path);
        if is_text {
            println!("\nContent:");
            match std::fs::read_to_string(local_path) {
                Ok(content) => {
                    let preview: String = content.chars().take(5000).collect();
                    println!("{}", preview);
                    if content.len() > 5000 {
                        println!("\n... (truncated, {} bytes total)", content.len());
                    }
                }
                Err(e) => println!("Failed to read: {}", e),
            }
        }
    } else {
        let mut result = serde_json::json!({
            "filename": attachment.filename,
            "mime_type": attachment.mime_type,
            "size": attachment.size,
            "downloaded": true,
            "path": local_path.to_string_lossy(),
        });

        if is_text {
            if let Ok(content) = std::fs::read_to_string(local_path) {
                result["content"] = serde_json::Value::String(content);
            }
        }

        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    Ok(())
}

// ============================================================================
// Calendar Create Function
// ============================================================================

async fn calendar_create(
    account: &str,
    summary: &str,
    start: &str,
    end: &str,
    description: Option<&str>,
    location: Option<&str>,
    attendees: Option<Vec<String>>,
    calendar_id: &str,
    human: bool,
) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let account_email = accounts
        .iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone())
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", account))?;

    // Build event body
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

    if let Some(att) = &attendees {
        if !att.is_empty() {
            event_body["attendees"] = serde_json::json!(
                att.iter().map(|email| serde_json::json!({"email": email})).collect::<Vec<_>>()
            );
        }
    }

    // Get access token
    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(&account_email).await?;

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
        if human {
            println!("❌ Failed to create event: {} - {}", status, error_body);
        } else {
            println!("{{\"success\": false, \"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let created_event: serde_json::Value = response.json().await?;
    let event_id = created_event["id"].as_str().unwrap_or("unknown");
    let html_link = created_event["htmlLink"].as_str();

    if human {
        println!("✅ Event created successfully!");
        println!("   Title: {}", summary);
        println!("   When: {} to {}", start, end);
        if let Some(loc) = location {
            println!("   Location: {}", loc);
        }
        if let Some(link) = html_link {
            println!("   Link: {}", link);
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "success": true,
            "event": {
                "id": event_id,
                "summary": summary,
                "start": start,
                "end": end,
                "calendar_id": calendar_id,
                "account": account_email,
                "html_link": html_link
            }
        }))?);
    }

    Ok(())
}

// ============================================================================
// Sync Management Functions
// ============================================================================

async fn sync_reset(account: &str, data_type: &str, confirm: bool, human: bool) -> Result<()> {
    if !["email", "calendar", "all"].contains(&data_type) {
        if human {
            println!("❌ Invalid data_type. Must be 'email', 'calendar', or 'all'");
        } else {
            println!("{{\"success\": false, \"error\": \"data_type must be 'email', 'calendar', or 'all'\"}}");
        }
        return Ok(());
    }

    if !confirm {
        if human {
            println!("❌ Must pass --confirm to reset sync data.");
            println!("\nThis will permanently delete synced {} data for this account.", data_type);
            println!("Example: groundeffect sync reset --account {} --confirm", account);
        } else {
            println!("{{\"success\": false, \"error\": \"Must pass --confirm to reset\"}}");
        }
        return Ok(());
    }

    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let email = accounts
        .iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone());

    let email = match email {
        Some(e) => e,
        None => {
            if human {
                println!("❌ Account not found: {}", account);
            } else {
                println!("{{\"success\": false, \"error\": \"Account not found\"}}");
            }
            return Ok(());
        }
    };

    // Clear sync data based on type
    let (email_count, event_count) = match data_type {
        "email" => {
            let count = db.clear_account_emails(&email).await?;
            (count, 0)
        }
        "calendar" => {
            let count = db.clear_account_events(&email).await?;
            (0, count)
        }
        _ => db.clear_account_sync_data(&email).await?,
    };

    // Reset account sync timestamps
    if let Some(mut acct) = db.get_account(&email).await? {
        match data_type {
            "email" => {
                acct.last_sync_email = None;
                acct.oldest_email_synced = None;
            }
            "calendar" => {
                acct.last_sync_calendar = None;
                acct.oldest_event_synced = None;
            }
            _ => {
                acct.last_sync_email = None;
                acct.last_sync_calendar = None;
                acct.oldest_email_synced = None;
                acct.oldest_event_synced = None;
            }
        }
        db.upsert_account(&acct).await?;
    }

    if human {
        println!("✅ Reset {} sync data for {}", data_type, email);
        println!("   {} emails deleted", email_count);
        println!("   {} events deleted", event_count);
        println!("\nRestart the daemon to re-sync.");
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "success": true,
            "message": format!("Reset {} sync data for {}", data_type, email),
            "deleted": {
                "emails": email_count,
                "events": event_count
            }
        }))?);
    }

    Ok(())
}

async fn sync_extend(account: &str, target_date: &str, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let email = accounts
        .iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone());

    let email = match email {
        Some(e) => e,
        None => {
            if human {
                println!("❌ Account not found: {}", account);
            } else {
                println!("{{\"success\": false, \"error\": \"Account not found\"}}");
            }
            return Ok(());
        }
    };

    let acct = db.get_account(&email).await?.ok_or_else(|| anyhow::anyhow!("Account not found"))?;

    let current_sync_from = acct.sync_email_since
        .unwrap_or_else(|| Utc::now() - chrono::Duration::days(90));

    // Parse target date
    let parsed_date = NaiveDate::parse_from_str(target_date, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("Invalid date format: {}. Use YYYY-MM-DD", e))?;

    let target_datetime = parsed_date
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Utc).single())
        .ok_or_else(|| anyhow::anyhow!("Failed to parse date"))?;

    if target_datetime >= current_sync_from {
        if human {
            println!("❌ Target date {} is already within current sync range (back to {})",
                target_date, current_sync_from.format("%Y-%m-%d"));
            println!("Choose an earlier date.");
        } else {
            println!("{{\"success\": false, \"error\": \"Target date must be earlier than current sync_from\"}}");
        }
        return Ok(());
    }

    // Update account
    let mut updated = acct.clone();
    updated.sync_email_since = Some(target_datetime);
    db.upsert_account(&updated).await?;

    let additional_days = (current_sync_from - target_datetime).num_days();

    // Automatically restart daemon to pick up new sync range
    let restart_method = restart_daemon();

    if human {
        println!("✅ Extended sync range for {}", email);
        println!("   Previous: {}", current_sync_from.format("%Y-%m-%d"));
        println!("   New: {}", target_date);
        println!("   Additional days: {}", additional_days);
        match restart_method {
            Some(method) => println!("\n✓ Daemon restarted via {}", method),
            None => println!("\nNote: Daemon not running. Start it to sync older data."),
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "success": true,
            "account": email,
            "sync_range": {
                "previous_sync_from": current_sync_from.format("%Y-%m-%d").to_string(),
                "new_sync_from": target_date,
                "additional_days": additional_days
            },
            "daemon_restarted": restart_method.is_some(),
            "restart_method": restart_method
        }))?);
    }

    Ok(())
}

async fn sync_resume_from(account: &str, target_date: &str, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let email = accounts
        .iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone());

    let email = match email {
        Some(e) => e,
        None => {
            if human {
                println!("❌ Account not found: {}", account);
            } else {
                println!("{{\"success\": false, \"error\": \"Account not found\"}}");
            }
            return Ok(());
        }
    };

    // Parse target date
    let parsed_date = NaiveDate::parse_from_str(target_date, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("Invalid date format: {}. Use YYYY-MM-DD", e))?;

    let target_datetime = parsed_date
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Utc).single())
        .ok_or_else(|| anyhow::anyhow!("Failed to parse date"))?;

    let acct = db.get_account(&email).await?.ok_or_else(|| anyhow::anyhow!("Account not found"))?;

    let old_oldest_email = acct.oldest_email_synced;
    let old_oldest_event = acct.oldest_event_synced;

    // Update account
    let mut updated = acct.clone();
    updated.oldest_email_synced = Some(target_datetime);
    updated.oldest_event_synced = Some(target_datetime);
    updated.last_sync_email = None;
    updated.last_sync_calendar = None;
    db.upsert_account(&updated).await?;

    if human {
        println!("✅ Sync will resume from {} for {}", target_date, email);
        if let Some(old) = old_oldest_email {
            println!("   Previous oldest email: {}", old.format("%Y-%m-%d"));
        }
        if let Some(old) = old_oldest_event {
            println!("   Previous oldest event: {}", old.format("%Y-%m-%d"));
        }
        println!("\nExisting data is preserved. Restart the daemon to apply changes.");
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "success": true,
            "account": email,
            "resume_from": target_date,
            "previous_state": {
                "oldest_email_synced": old_oldest_email.map(|d| d.format("%Y-%m-%d").to_string()),
                "oldest_event_synced": old_oldest_event.map(|d| d.format("%Y-%m-%d").to_string())
            }
        }))?);
    }

    Ok(())
}

async fn sync_download_attachments(account: &str, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    // Resolve account
    let email = accounts
        .iter()
        .find(|a| a.id == account || a.alias.as_ref() == Some(&account.to_string()))
        .map(|a| a.id.clone());

    let email = match email {
        Some(e) => e,
        None => {
            if human {
                println!("❌ Account not found: {}", account);
            } else {
                println!("{{\"success\": false, \"error\": \"Account not found\"}}");
            }
            return Ok(());
        }
    };

    // Get attachment stats
    let (total, downloaded, _) = db.get_attachment_stats(&email).await.unwrap_or((0, 0, 0));
    let pending = total - downloaded;

    if pending == 0 {
        if human {
            println!("✅ No pending attachments for {}", email);
            println!("   Total attachments: {}", total);
            println!("   Downloaded: {}", downloaded);
        } else {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "success": true,
                "account": email,
                "pending_count": 0,
                "message": "No pending attachments"
            }))?);
        }
        return Ok(());
    }

    let acct = db.get_account(&email).await?.ok_or_else(|| anyhow::anyhow!("Account not found"))?;

    // Enable sync_attachments if not already
    let was_enabled = acct.sync_attachments;
    if !was_enabled {
        let mut updated = acct.clone();
        updated.sync_attachments = true;
        db.upsert_account(&updated).await?;
    }

    if human {
        println!("✅ Attachment download enabled for {}", email);
        println!("   Pending attachments: {}", pending);
        if !was_enabled {
            println!("   sync_attachments: enabled (was disabled)");
        } else {
            println!("   sync_attachments: already enabled");
        }
        println!("\nThe daemon will download attachments in the background.");
        println!("Restart the daemon if it's not running.");
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "success": true,
            "account": email,
            "pending_count": pending,
            "sync_attachments_enabled": true,
            "message": if was_enabled {
                "sync_attachments already enabled - daemon will download in background"
            } else {
                "Enabled sync_attachments - daemon will download in background"
            }
        }))?);
    }

    Ok(())
}

// ============================================================================
// Config Settings Function
// ============================================================================

async fn config_settings(
    logging: Option<bool>,
    email_interval: Option<u64>,
    calendar_interval: Option<u64>,
    max_fetches: Option<u32>,
    timezone: Option<String>,
    human: bool,
) -> Result<()> {
    let mut daemon_config = DaemonConfig::load().unwrap_or_default();
    let mut config = Config::load().unwrap_or_default();
    let mut changes = vec![];

    // Apply daemon config changes
    if let Some(l) = logging {
        if daemon_config.logging_enabled != l {
            daemon_config.logging_enabled = l;
            changes.push(format!("logging_enabled: {}", l));
        }
    }

    if let Some(interval) = email_interval {
        let clamped = interval.clamp(60, 3600);
        if daemon_config.email_poll_interval_secs != clamped {
            daemon_config.email_poll_interval_secs = clamped;
            changes.push(format!("email_poll_interval: {} seconds", clamped));
        }
    }

    if let Some(interval) = calendar_interval {
        let clamped = interval.clamp(60, 3600);
        if daemon_config.calendar_poll_interval_secs != clamped {
            daemon_config.calendar_poll_interval_secs = clamped;
            changes.push(format!("calendar_poll_interval: {} seconds", clamped));
        }
    }

    if let Some(fetches) = max_fetches {
        let clamped = fetches.clamp(1, 50) as usize;
        if daemon_config.max_concurrent_fetches != clamped {
            daemon_config.max_concurrent_fetches = clamped;
            changes.push(format!("max_concurrent_fetches: {}", clamped));
        }
    }

    // Apply general config changes (timezone)
    let mut general_config_changed = false;
    if let Some(tz) = &timezone {
        // Validate timezone by trying to parse it
        if tz.parse::<chrono_tz::Tz>().is_ok() || tz == "UTC" {
            if config.general.timezone != *tz {
                config.general.timezone = tz.clone();
                changes.push(format!("timezone: {}", tz));
                general_config_changed = true;
            }
        } else {
            return Err(anyhow::anyhow!(
                "Invalid timezone '{}'. Use IANA timezone names like 'America/Los_Angeles', 'Europe/London', or 'UTC'.",
                tz
            ));
        }
    }

    // Save configs if changes were made
    let daemon_config_changed = changes.iter().any(|c|
        c.starts_with("logging") || c.starts_with("email_poll") ||
        c.starts_with("calendar_poll") || c.starts_with("max_concurrent")
    );
    if daemon_config_changed {
        daemon_config.save()?;
    }
    if general_config_changed {
        config.save()?;
    }

    if human {
        println!("\n⚙️  Settings");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Timezone: {}", config.general.timezone);
        println!("Logging enabled: {}", daemon_config.logging_enabled);
        println!("Email poll interval: {} seconds", daemon_config.email_poll_interval_secs);
        println!("Calendar poll interval: {} seconds", daemon_config.calendar_poll_interval_secs);
        println!("Max concurrent fetches: {}", daemon_config.max_concurrent_fetches);
        println!("\nDaemon config: {:?}", DaemonConfig::config_path());

        if !changes.is_empty() {
            println!("\nChanges made:");
            for change in &changes {
                println!("  - {}", change);
            }
            println!("\nRestart the daemon for changes to take effect.");
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "settings": {
                "timezone": config.general.timezone,
                "logging_enabled": daemon_config.logging_enabled,
                "email_poll_interval_secs": daemon_config.email_poll_interval_secs,
                "calendar_poll_interval_secs": daemon_config.calendar_poll_interval_secs,
                "max_concurrent_fetches": daemon_config.max_concurrent_fetches,
            },
            "daemon_config_path": DaemonConfig::config_path().to_string_lossy(),
            "changes": changes
        }))?);
    }

    Ok(())
}

// ============================================================================
// Draft Command Handler
// ============================================================================

async fn handle_draft_command(command: DraftCommands, global_human: bool) -> Result<()> {
    match command {
        DraftCommands::Create { from, to, subject, body, cc, bcc, html, reply_to, human } => {
            let human = human || global_human;
            draft_create(&from, to, &subject, &body, cc, bcc, html, reply_to, human).await?;
        }
        DraftCommands::List { from, limit, human } => {
            let human = human || global_human;
            draft_list(&from, limit, human).await?;
        }
        DraftCommands::Show { from, draft_id, human } => {
            let human = human || global_human;
            draft_show(&from, &draft_id, human).await?;
        }
        DraftCommands::Update { from, draft_id, to, subject, body, cc, bcc, html, human } => {
            let human = human || global_human;
            draft_update(&from, &draft_id, to, subject, body, cc, bcc, html, human).await?;
        }
        DraftCommands::Send { from, draft_id, human } => {
            let human = human || global_human;
            draft_send(&from, &draft_id, human).await?;
        }
        DraftCommands::Delete { from, draft_id, human } => {
            let human = human || global_human;
            draft_delete(&from, &draft_id, human).await?;
        }
    }
    Ok(())
}

// ============================================================================
// Draft Functions
// ============================================================================

async fn draft_create(
    from: &str,
    to: Vec<String>,
    subject: &str,
    body: &str,
    cc: Option<Vec<String>>,
    bcc: Option<Vec<String>>,
    force_html: bool,
    reply_to: Option<String>,
    human: bool,
) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;
    let display_name = &account.display_name;

    let cc_list = cc.unwrap_or_default();
    let bcc_list = bcc.unwrap_or_default();

    let mut in_reply_to = None;
    let mut references = None;
    let mut final_subject = subject.to_string();

    if let Some(reply_id) = &reply_to {
        if let Ok(Some(original)) = db.get_email(reply_id).await {
            in_reply_to = Some(original.message_id.clone());
            references = Some(original.message_id.clone());
            if !final_subject.starts_with("Re:") && !final_subject.starts_with("RE:") {
                final_subject = format!("Re: {}", original.subject);
            }
        }
    }

    let is_html = force_html || detect_html_content(body);

    let message = build_email_message(
        display_name, from_email, &to, &cc_list, &bcc_list,
        &final_subject, body, is_html,
        in_reply_to.as_deref(), references.as_deref(),
    );

    let encoded = URL_SAFE_NO_PAD.encode(message.as_bytes());

    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    let response = client
        .post("https://gmail.googleapis.com/gmail/v1/users/me/drafts")
        .bearer_auth(&access_token)
        .json(&serde_json::json!({ "message": { "raw": encoded } }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to create draft: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let result: serde_json::Value = response.json().await?;
    let draft_id = result["id"].as_str().unwrap_or("unknown");
    let message_id = result["message"]["id"].as_str().unwrap_or("unknown");

    if human {
        println!("✅ Draft created successfully!");
        println!("   Draft ID: {}", draft_id);
        println!("   Subject: {}", final_subject);
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "status": "draft_created",
            "draft_id": draft_id,
            "message_id": message_id,
            "from": format!("{} <{}>", display_name, from_email),
            "to": to,
            "subject": final_subject,
        }))?);
    }

    Ok(())
}

async fn draft_list(from: &str, limit: usize, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;

    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts?maxResults={}", limit.min(100)))
        .bearer_auth(&access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to list drafts: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let result: serde_json::Value = response.json().await?;
    let drafts_array = result["drafts"].as_array();

    if drafts_array.is_none() || drafts_array.unwrap().is_empty() {
        if human {
            println!("📭 No drafts found.");
        } else {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "drafts": [], "total": 0 }))?);
        }
        return Ok(());
    }

    let mut drafts = Vec::new();
    if let Some(draft_list) = drafts_array {
        for draft in draft_list {
            let draft_id = draft["id"].as_str().unwrap_or("unknown");
            let draft_response = client
                .get(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=To&metadataHeaders=Date", draft_id))
                .bearer_auth(&access_token)
                .send()
                .await?;

            if draft_response.status().is_success() {
                let draft_data: serde_json::Value = draft_response.json().await?;
                let headers = draft_data["message"]["payload"]["headers"].as_array();
                let mut subject = String::new();
                let mut to_str = String::new();
                let mut date = String::new();

                if let Some(hdrs) = headers {
                    for h in hdrs {
                        match h["name"].as_str().unwrap_or("") {
                            "Subject" => subject = h["value"].as_str().unwrap_or("").to_string(),
                            "To" => to_str = h["value"].as_str().unwrap_or("").to_string(),
                            "Date" => date = h["value"].as_str().unwrap_or("").to_string(),
                            _ => {}
                        }
                    }
                }

                let snippet = draft_data["message"]["snippet"].as_str().unwrap_or("");
                drafts.push(serde_json::json!({
                    "draft_id": draft_id, "subject": subject, "to": to_str, "snippet": snippet, "date": date,
                }));
            }
        }
    }

    if human {
        println!("\n📝 Drafts ({}):\n", drafts.len());
        for d in &drafts {
            println!("📧 {}", d["subject"].as_str().unwrap_or("(no subject)"));
            println!("   To: {}", d["to"].as_str().unwrap_or(""));
            println!("   Draft ID: {}", d["draft_id"].as_str().unwrap_or(""));
            println!();
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "drafts": drafts, "total": drafts.len() }))?);
    }

    Ok(())
}

async fn draft_show(from: &str, draft_id: &str, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;

    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=full", draft_id))
        .bearer_auth(&access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to get draft: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let draft_data: serde_json::Value = response.json().await?;
    let headers = draft_data["message"]["payload"]["headers"].as_array();
    let mut subject = String::new();
    let mut to = String::new();
    let mut cc = String::new();
    let mut from_header = String::new();
    let mut date = String::new();

    if let Some(hdrs) = headers {
        for h in hdrs {
            match h["name"].as_str().unwrap_or("") {
                "Subject" => subject = h["value"].as_str().unwrap_or("").to_string(),
                "To" => to = h["value"].as_str().unwrap_or("").to_string(),
                "Cc" => cc = h["value"].as_str().unwrap_or("").to_string(),
                "From" => from_header = h["value"].as_str().unwrap_or("").to_string(),
                "Date" => date = h["value"].as_str().unwrap_or("").to_string(),
                _ => {}
            }
        }
    }

    // Recursively extract text/plain body from potentially nested multipart structures
    fn extract_text_body(part: &serde_json::Value) -> Option<String> {
        let mime_type = part["mimeType"].as_str().unwrap_or("");

        if mime_type == "text/plain" {
            if let Some(body_data) = part["body"]["data"].as_str() {
                // Gmail uses URL-safe base64 - try with padding first, then without
                use base64::{Engine, engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD}};
                let decoded = URL_SAFE.decode(body_data)
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
                    if let Some(text) = extract_text_body(nested_part) {
                        return Some(text);
                    }
                }
            }
        }
        None
    }

    let payload = &draft_data["message"]["payload"];
    let body = extract_text_body(payload).unwrap_or_default();

    if human {
        println!("\n📝 Draft: {}", draft_id);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("From: {}", from_header);
        println!("To: {}", to);
        if !cc.is_empty() { println!("CC: {}", cc); }
        println!("Subject: {}", subject);
        println!("Date: {}", date);
        println!("\n{}", body);
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "draft_id": draft_id, "from": from_header, "to": to, "cc": cc,
            "subject": subject, "body": body, "date": date,
        }))?);
    }

    Ok(())
}

async fn draft_update(
    from: &str, draft_id: &str, to: Option<Vec<String>>, subject: Option<String>,
    body: Option<String>, cc: Option<Vec<String>>, bcc: Option<Vec<String>>,
    force_html: bool, human: bool,
) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;
    let display_name = &account.display_name;

    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    // Get existing draft
    let existing_response = client
        .get(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=full", draft_id))
        .bearer_auth(&access_token)
        .send()
        .await?;

    if !existing_response.status().is_success() {
        let status = existing_response.status();
        let error_body = existing_response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to get draft: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let existing: serde_json::Value = existing_response.json().await?;
    let headers = existing["message"]["payload"]["headers"].as_array();
    let mut existing_subject = String::new();
    let mut existing_to = String::new();
    let mut existing_cc = String::new();
    let mut existing_body = String::new();

    if let Some(hdrs) = headers {
        for h in hdrs {
            match h["name"].as_str().unwrap_or("") {
                "Subject" => existing_subject = h["value"].as_str().unwrap_or("").to_string(),
                "To" => existing_to = h["value"].as_str().unwrap_or("").to_string(),
                "Cc" => existing_cc = h["value"].as_str().unwrap_or("").to_string(),
                _ => {}
            }
        }
    }

    let payload = &existing["message"]["payload"];
    let mime_type = payload["mimeType"].as_str().unwrap_or("");
    if mime_type.starts_with("multipart/") {
        if let Some(parts) = payload["parts"].as_array() {
            for part in parts {
                if part["mimeType"].as_str().unwrap_or("") == "text/plain" {
                    if let Some(body_data) = part["body"]["data"].as_str() {
                        if let Ok(decoded) = URL_SAFE_NO_PAD.decode(body_data) {
                            if let Ok(text) = String::from_utf8(decoded) {
                                existing_body = text;
                                break;
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(body_data) = payload["body"]["data"].as_str() {
        if let Ok(decoded) = URL_SAFE_NO_PAD.decode(body_data) {
            if let Ok(text) = String::from_utf8(decoded) {
                existing_body = text;
            }
        }
    }

    let final_to: Vec<String> = to.unwrap_or_else(|| existing_to.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
    let final_subject = subject.unwrap_or(existing_subject);
    let final_body = body.unwrap_or(existing_body);
    let final_cc: Vec<String> = cc.unwrap_or_else(|| existing_cc.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
    let final_bcc: Vec<String> = bcc.unwrap_or_default();

    let is_html = force_html || detect_html_content(&final_body);

    let message = build_email_message(
        display_name, from_email, &final_to, &final_cc, &final_bcc,
        &final_subject, &final_body, is_html, None, None,
    );

    let encoded = URL_SAFE_NO_PAD.encode(message.as_bytes());

    let response = client
        .put(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}", draft_id))
        .bearer_auth(&access_token)
        .json(&serde_json::json!({ "message": { "raw": encoded } }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to update draft: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let result: serde_json::Value = response.json().await?;
    let new_draft_id = result["id"].as_str().unwrap_or(draft_id);

    if human {
        println!("✅ Draft updated successfully!");
        println!("   Draft ID: {}", new_draft_id);
        println!("   Subject: {}", final_subject);
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "status": "updated", "draft_id": new_draft_id,
            "from": format!("{} <{}>", display_name, from_email),
            "to": final_to, "subject": final_subject,
        }))?);
    }

    Ok(())
}

async fn draft_send(from: &str, draft_id: &str, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;

    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    // Get draft info
    let draft_response = client
        .get(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=To", draft_id))
        .bearer_auth(&access_token)
        .send()
        .await?;

    let mut subject = String::new();
    let mut to = String::new();
    if draft_response.status().is_success() {
        let draft_data: serde_json::Value = draft_response.json().await?;
        if let Some(hdrs) = draft_data["message"]["payload"]["headers"].as_array() {
            for h in hdrs {
                match h["name"].as_str().unwrap_or("") {
                    "Subject" => subject = h["value"].as_str().unwrap_or("").to_string(),
                    "To" => to = h["value"].as_str().unwrap_or("").to_string(),
                    _ => {}
                }
            }
        }
    }

    let response = client
        .post("https://gmail.googleapis.com/gmail/v1/users/me/drafts/send")
        .bearer_auth(&access_token)
        .json(&serde_json::json!({ "id": draft_id }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to send draft: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    let result: serde_json::Value = response.json().await?;
    let message_id = result["id"].as_str().unwrap_or("unknown");

    if human {
        println!("✅ Draft sent successfully!");
        println!("   Message ID: {}", message_id);
        println!("   To: {}", to);
        println!("   Subject: {}", subject);
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "status": "sent", "message_id": message_id, "draft_id": draft_id, "to": to, "subject": subject,
        }))?);
    }

    Ok(())
}

async fn draft_delete(from: &str, draft_id: &str, human: bool) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let token_provider = create_token_provider(&config).await?;
    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    let account = accounts
        .iter()
        .find(|a| a.id == from || a.alias.as_ref() == Some(&from.to_string()))
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", from))?;
    let from_email = &account.id;

    let oauth = OAuthManager::new(token_provider);
    let access_token = oauth.get_valid_token(from_email).await?;
    let client = reqwest::Client::new();

    let response = client
        .delete(format!("https://gmail.googleapis.com/gmail/v1/users/me/drafts/{}", draft_id))
        .bearer_auth(&access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_default();
        if human {
            println!("❌ Failed to delete draft: {} - {}", status, error_body);
        } else {
            println!("{{\"error\": \"{} - {}\"}}", status, error_body.replace('"', "\\\""));
        }
        return Ok(());
    }

    if human {
        println!("✅ Draft deleted successfully!");
        println!("   Draft ID: {}", draft_id);
    } else {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "status": "deleted", "draft_id": draft_id }))?);
    }

    Ok(())
}

// ============================================================================
// Email Helper Functions
// ============================================================================

fn detect_html_content(body: &str) -> bool {
    use regex::Regex;
    let md_link = Regex::new(r"\[.+?\]\(.+?\)").unwrap();
    if md_link.is_match(body) { return true; }
    let url_pattern = Regex::new(r"https?://[^\s]+").unwrap();
    if url_pattern.is_match(body) { return true; }
    let bold = Regex::new(r"\*\*.+?\*\*|__.+?__").unwrap();
    if bold.is_match(body) { return true; }
    // Simple italic detection - single asterisk not part of bold
    let italic = Regex::new(r"(?:^|[^*])\*[^*\n]+?\*(?:[^*]|$)").unwrap();
    if italic.is_match(body) { return true; }
    let html_tag = Regex::new(r"</?[a-zA-Z][^>]*>").unwrap();
    if html_tag.is_match(body) { return true; }
    false
}

fn convert_to_html(body: &str) -> String {
    use regex::Regex;
    let mut html = body.to_string();
    // Convert markdown links first
    let md_link = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
    html = md_link.replace_all(&html, r#"<a href="$2">$1</a>"#).to_string();
    // Convert plain URLs not already in href (preceded by " or >)
    let url_pattern = Regex::new(r#"(^|[^">])(https?://[^\s<>"]+)"#).unwrap();
    html = url_pattern.replace_all(&html, r#"$1<a href="$2">$2</a>"#).to_string();
    // Convert bold first (** and __)
    let bold = Regex::new(r"\*\*(.+?)\*\*").unwrap();
    html = bold.replace_all(&html, r"<strong>$1</strong>").to_string();
    let bold2 = Regex::new(r"__(.+?)__").unwrap();
    html = bold2.replace_all(&html, r"<strong>$1</strong>").to_string();
    // Convert italic (single * or _) - safe now since ** and __ are converted
    let italic = Regex::new(r"\*([^*\n]+?)\*").unwrap();
    html = italic.replace_all(&html, r"<em>$1</em>").to_string();
    let italic2 = Regex::new(r"_([^_\n]+?)_").unwrap();
    html = italic2.replace_all(&html, r"<em>$1</em>").to_string();
    html = html.replace("\n", "<br>\n");
    html
}

fn strip_html_tags(html: &str) -> String {
    use regex::Regex;
    let mut text = html.to_string();
    let br_tag = Regex::new(r"<br\s*/?>").unwrap();
    text = br_tag.replace_all(&text, "\n").to_string();
    text = text.replace("</p>", "\n\n");
    let anchor = Regex::new(r#"<a[^>]+href="([^"]+)"[^>]*>([^<]+)</a>"#).unwrap();
    text = anchor.replace_all(&text, "$2 ($1)").to_string();
    let tag = Regex::new(r"<[^>]+>").unwrap();
    text = tag.replace_all(&text, "").to_string();
    let multi_newline = Regex::new(r"\n{3,}").unwrap();
    text = multi_newline.replace_all(&text, "\n\n").to_string();
    text.trim().to_string()
}

fn encode_display_name(name: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let needs_encoding = name.chars().any(|c| !c.is_ascii() || c == '"' || c == '\\');
    if !needs_encoding {
        if name.contains(' ') || name.contains(',') || name.contains('<') || name.contains('>') {
            return format!("\"{}\"", name);
        }
        return name.to_string();
    }
    let encoded = STANDARD.encode(name.as_bytes());
    format!("=?UTF-8?B?{}?=", encoded)
}

fn build_email_message(
    display_name: &str, from_email: &str, to: &[String], cc: &[String], bcc: &[String],
    subject: &str, body: &str, is_html: bool, in_reply_to: Option<&str>, references: Option<&str>,
) -> String {
    let encoded_name = encode_display_name(display_name);
    let from_header = format!("{} <{}>", encoded_name, from_email);

    let mut message = format!("From: {}\r\nTo: {}\r\n", from_header, to.join(", "));

    if !cc.is_empty() { message.push_str(&format!("Cc: {}\r\n", cc.join(", "))); }
    if !bcc.is_empty() { message.push_str(&format!("Bcc: {}\r\n", bcc.join(", "))); }
    if let Some(msg_id) = in_reply_to { message.push_str(&format!("In-Reply-To: {}\r\n", msg_id)); }
    if let Some(refs) = references { message.push_str(&format!("References: {}\r\n", refs)); }

    message.push_str(&format!("Subject: {}\r\nMIME-Version: 1.0\r\n", subject));

    if is_html {
        let boundary = format!("----=_Part_{}", chrono::Utc::now().timestamp_millis());
        let html_body = convert_to_html(body);
        let plain_body = strip_html_tags(&html_body);

        message.push_str(&format!("Content-Type: multipart/alternative; boundary=\"{}\"\r\n\r\n", boundary));
        message.push_str(&format!("--{}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\n{}\r\n\r\n", boundary, plain_body));
        message.push_str(&format!("--{}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\n{}\r\n\r\n", boundary, html_body));
        message.push_str(&format!("--{}--\r\n", boundary));
    } else {
        message.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
        message.push_str(body);
    }

    message
}
