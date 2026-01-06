//! GroundEffect CLI
//!
//! Full-featured command-line interface for managing and querying GroundEffect.

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use groundeffect_core::config::Config;
use groundeffect_core::db::Database;
use groundeffect_core::embedding::{EmbeddingEngine, EmbeddingModel};
use groundeffect_core::models::{Account, AccountStatus, CalendarEvent, Email, EventTime};
use groundeffect_core::search::{CalendarSearchOptions, SearchEngine, SearchOptions};

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

            // Initialize embedding engine for vector search
            let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
                .unwrap_or(EmbeddingModel::MiniLML6);
            let embedding = Arc::new(
                EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_metal)?
            );

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
            options.date_from = parse_date(&after);
            options.date_to = parse_date(&before);
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

            // Initialize embedding engine
            let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
                .unwrap_or(EmbeddingModel::MiniLML6);
            let embedding = Arc::new(
                EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_metal)?
            );

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
                date_from: parse_date(&after),
                date_to: parse_date(&before),
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
                        if let Some(desc) = &event.description {
                            println!("\n{}", desc);
                        }
                        if !event.attendees.is_empty() {
                            println!("\nAttendees:");
                            for attendee in &event.attendees {
                                let name = attendee.name.as_deref().unwrap_or(&attendee.email);
                                println!("  - {}", name);
                            }
                        }
                    } else {
                        #[derive(Serialize)]
                        struct EventDetail {
                            id: String,
                            summary: String,
                            start: String,
                            end: String,
                            location: Option<String>,
                            description: Option<String>,
                            attendees: Vec<String>,
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
                            attendees: event.attendees.iter().map(|a| a.email.clone()).collect(),
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
                    println!("   📨 Emails: {}", email_count);
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

fn resolve_account(accounts: &[Account], query: &str) -> Option<String> {
    accounts
        .iter()
        .find(|a| a.id == query || a.alias.as_ref() == Some(&query.to_string()))
        .map(|a| a.id.clone())
}

fn parse_date(date_str: &Option<String>) -> Option<DateTime<Utc>> {
    date_str.as_ref().and_then(|s| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
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
// Config Command Handlers
// ============================================================================

async fn handle_config_command(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::AddPermissions => config_add_permissions().await,
        ConfigCommands::RemovePermissions => config_remove_permissions().await,
    }
}

async fn config_add_permissions() -> Result<()> {
    use std::fs;
    use std::path::PathBuf;

    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
    let settings_path = PathBuf::from(&home).join(".claude").join("settings.json");

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
        return Ok(());
    }

    allow_list.push(serde_json::json!(permission));

    let content = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_path, content)?;

    println!("✓ Added groundeffect to Claude Code allowlist");
    println!("  {}", settings_path.display());

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
