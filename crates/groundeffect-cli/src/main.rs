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
#[command(about = "GroundEffect email/calendar sync CLI", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Human-readable output (default is JSON)
    #[arg(long, global = true)]
    human: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Email commands
    Email {
        #[command(subcommand)]
        command: EmailCommands,
    },
    /// Calendar commands
    Calendar {
        #[command(subcommand)]
        command: CalendarCommands,
    },
    /// Account management
    Account {
        #[command(subcommand)]
        command: AccountCommands,
    },
    /// Sync management
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },
    /// Daemon management
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
}

// ============================================================================
// Email Commands
// ============================================================================

#[derive(Subcommand)]
enum EmailCommands {
    /// Search emails using hybrid BM25 + vector semantic search
    Search {
        /// Search query (natural language)
        query: String,
        /// Filter by sender email/name
        #[arg(long)]
        from: Option<String>,
        /// Filter by recipient
        #[arg(long)]
        to: Option<String>,
        /// Emails after date (YYYY-MM-DD)
        #[arg(long)]
        after: Option<String>,
        /// Emails before date (YYYY-MM-DD)
        #[arg(long)]
        before: Option<String>,
        /// Filter by IMAP folder
        #[arg(long)]
        folder: Option<String>,
        /// Only emails with attachments
        #[arg(long)]
        has_attachment: bool,
        /// Filter to specific account(s)
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Number of results (default 10, max 100)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// List recent emails
    List {
        /// Filter to specific account
        #[arg(long)]
        account: Option<String>,
        /// Number of emails (default 10, max 100)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Show single email by ID
    Show {
        /// Email ID
        id: String,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Show email thread
    Thread {
        /// Thread ID
        thread_id: String,
        /// Filter to specific accounts
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Calendar Commands
// ============================================================================

#[derive(Subcommand)]
enum CalendarCommands {
    /// Search calendar events
    Search {
        /// Search query (natural language)
        query: String,
        /// Events after date (YYYY-MM-DD)
        #[arg(long)]
        after: Option<String>,
        /// Events before date (YYYY-MM-DD)
        #[arg(long)]
        before: Option<String>,
        /// Filter by calendar ID
        #[arg(long)]
        calendar: Option<String>,
        /// Filter to specific account(s)
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Number of results (default 10, max 100)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// List all calendars
    List {
        /// Filter to specific account(s)
        #[arg(long)]
        account: Option<Vec<String>>,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Show single calendar event
    Show {
        /// Event ID
        id: String,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Account Commands
// ============================================================================

#[derive(Subcommand)]
enum AccountCommands {
    /// List all accounts
    List {
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Show account details
    Show {
        /// Account email or alias
        account: String,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Sync Commands
// ============================================================================

#[derive(Subcommand)]
enum SyncCommands {
    /// Show sync status
    Status {
        /// Show status for specific account
        #[arg(long)]
        account: Option<String>,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
}

// ============================================================================
// Daemon Commands
// ============================================================================

#[derive(Subcommand)]
enum DaemonCommands {
    /// Check daemon status
    Status {
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Start the daemon
    Start {
        /// Enable file logging
        #[arg(long)]
        logging: bool,
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Stop the daemon
    Stop {
        /// Human-readable output
        #[arg(long)]
        human: bool,
    },
    /// Restart the daemon
    Restart {
        /// Enable file logging
        #[arg(long)]
        logging: bool,
        /// Human-readable output
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
                    if att_total > 0 {
                        println!("   📎 Attachments: {}/{} downloaded ({})", att_downloaded, att_total, format_bytes(att_size));
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
