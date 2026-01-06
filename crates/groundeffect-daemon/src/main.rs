//! GroundEffect Daemon
//!
//! Long-running launchd service that handles email/calendar sync,
//! indexing, and writes to LanceDB.

use std::process::Command;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use dialoguer::{Confirm, Input, Select};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{error, info, warn, Level};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use groundeffect_core::config::{Config, DaemonConfig};
use groundeffect_core::db::Database;
use groundeffect_core::embedding::{EmbeddingEngine, EmbeddingModel};
use groundeffect_core::keychain::KeychainManager;
use groundeffect_core::mcp::McpServer;
use groundeffect_core::models::{Account, AccountStatus};
use groundeffect_core::oauth::OAuthManager;
use groundeffect_core::sync::{SyncEvent, SyncManager, SyncType};

#[derive(Parser)]
#[command(name = "groundeffect-daemon")]
#[command(about = "GroundEffect email/calendar sync daemon", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Enable file logging to ~/.groundeffect/logs/daemon.log
    #[arg(long, global = true)]
    log: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a new Google account via OAuth
    AddAccount {
        /// Optional alias for the account (e.g., "work", "personal")
        #[arg(short, long)]
        alias: Option<String>,
    },
    /// List all configured accounts
    ListAccounts,
    /// Remove an account
    RemoveAccount {
        /// Account email address or alias
        account: String,
    },
    /// Run the daemon (default if no command specified)
    Run,
    /// Run as MCP server (stdio JSON-RPC for Claude Code)
    Mcp,
    /// Interactive setup wizard - configure settings and optionally install launchd agent
    Setup {
        /// Install launchd agent for auto-start at login
        #[arg(long)]
        install: bool,
        /// Uninstall launchd agent
        #[arg(long)]
        uninstall: bool,
    },
    /// Interactive configuration - change daemon settings
    Configure,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging (but not for MCP mode - it uses stdio for JSON-RPC)
    let is_mcp = matches!(cli.command, Some(Commands::Mcp));
    if !is_mcp {
        // Check CLI flag OR environment variable for logging
        let enable_logging = cli.log
            || std::env::var("GROUNDEFFECT_DAEMON_LOGGING")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);

        if enable_logging {
            // File logging to XDG data directory (~/.local/share/groundeffect/logs)
            let log_dir = dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
                .join("groundeffect")
                .join("logs");
            std::fs::create_dir_all(&log_dir).ok();

            let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "daemon.log");

            // Filter out noisy LanceDB internal logs
            let filter = tracing_subscriber::filter::EnvFilter::new(
                "info,lance=warn,lancedb=warn,lance_core=warn,lance_index=warn,lance_table=warn,lance_file=warn,lance_encoding=warn"
            );

            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(file_appender)
                .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f".to_string()))
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true)
                .with_span_events(FmtSpan::CLOSE);

            tracing_subscriber::registry()
                .with(file_layer.with_filter(filter))
                .init();

            // Log where logs are being written
            info!("Logging to {:?}", log_dir.join("daemon.log"));
        } else {
            // Console logging (default)
            let _subscriber = tracing_subscriber::fmt()
                .with_max_level(Level::INFO)
                .with_span_events(FmtSpan::CLOSE)
                .with_target(true)
                .with_thread_ids(true)
                .init();
        }
    }

    match cli.command {
        Some(Commands::AddAccount { alias }) => add_account(alias).await,
        Some(Commands::ListAccounts) => list_accounts().await,
        Some(Commands::RemoveAccount { account }) => remove_account(&account).await,
        Some(Commands::Run) | None => run_daemon().await,
        Some(Commands::Mcp) => run_mcp_server().await,
        Some(Commands::Setup { install, uninstall }) => run_setup(install, uninstall),
        Some(Commands::Configure) => run_configure(),
    }
}

/// Add a new Google account via OAuth flow
async fn add_account(alias: Option<String>) -> Result<()> {
    info!("Starting OAuth flow to add a new account...");

    // Check for OAuth credentials
    let client_id = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_ID");
    let client_secret = std::env::var("GROUNDEFFECT_GOOGLE_CLIENT_SECRET");

    if client_id.is_err() || client_secret.is_err() {
        eprintln!("\n❌ OAuth credentials not configured!\n");
        eprintln!("Please set the following environment variables:");
        eprintln!("  export GROUNDEFFECT_GOOGLE_CLIENT_ID=\"your-client-id\"");
        eprintln!("  export GROUNDEFFECT_GOOGLE_CLIENT_SECRET=\"your-client-secret\"");
        eprintln!("\nYou can get these from the Google Cloud Console:");
        eprintln!("  https://console.cloud.google.com/apis/credentials");
        eprintln!("\nMake sure to:");
        eprintln!("  1. Create an OAuth 2.0 Client ID (Desktop app type)");
        eprintln!("  2. Add http://localhost:8085/oauth/callback as a redirect URI");
        eprintln!("  3. Enable Gmail API and Google Calendar API\n");
        return Ok(());
    }

    let oauth = OAuthManager::new();

    // Generate state for CSRF protection
    let state = format!("groundeffect_{}", uuid::Uuid::new_v4());

    // Generate authorization URL
    let auth_url = oauth.authorization_url(&state);

    println!("\n🔐 Opening browser for Google authentication...\n");
    println!("If the browser doesn't open, visit this URL manually:");
    println!("{}\n", auth_url);

    // Open browser
    if let Err(e) = open::that(&auth_url) {
        warn!("Failed to open browser: {}", e);
    }

    // Start local HTTP server to receive callback
    let listener = TcpListener::bind("127.0.0.1:8085").await?;
    println!("⏳ Waiting for authentication callback on http://localhost:8085 ...\n");

    // Accept one connection
    let (mut socket, _) = listener.accept().await?;

    // Read the HTTP request
    let mut reader = BufReader::new(&mut socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    // Parse the request to extract code and state
    let (code, received_state) = parse_oauth_callback(&request_line)?;

    // Verify state
    if received_state != state {
        let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<h1>Error: Invalid state</h1>";
        socket.write_all(response.as_bytes()).await?;
        anyhow::bail!("OAuth state mismatch - possible CSRF attack");
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
    <p>You can close this window and return to the terminal.</p>
</body>
</html>"#;
    socket.write_all(success_html.as_bytes()).await?;
    drop(socket);

    println!("✅ Received authorization code, exchanging for tokens...\n");

    // Exchange code for tokens
    let (tokens, user_info) = oauth.exchange_code(&code).await?;

    // Store tokens in keychain
    KeychainManager::store_tokens(&user_info.email, &tokens)?;

    println!("✅ Tokens stored securely\n");

    // Load config and open database
    let config = Config::load().unwrap_or_default();
    std::fs::create_dir_all(config.lancedb_dir())?;
    let db = Database::open(config.lancedb_dir()).await?;

    // Check if account already exists
    if let Some(existing) = db.get_account(&user_info.email).await? {
        println!("ℹ️  Account {} already exists, updating tokens...", existing.id);
        let mut updated = existing;
        updated.status = AccountStatus::Active;
        updated.alias = alias.or(updated.alias);
        db.upsert_account(&updated).await?;
    } else {
        // Create new account with default 1 year sync
        use chrono::Duration;
        let account = Account {
            id: user_info.email.clone(),
            alias,
            display_name: user_info.name.unwrap_or_else(|| user_info.email.clone()),
            added_at: Utc::now(),
            last_sync_email: None,
            last_sync_calendar: None,
            status: AccountStatus::Active,
            sync_email_since: Some(Utc::now() - Duration::days(365)),
            oldest_email_synced: None,
            oldest_event_synced: None,
            sync_attachments: false,  // Off by default
        };
        db.upsert_account(&account).await?;
        println!("✅ Account created: {}", account.id);
    }

    println!("\n🎉 Successfully added account: {}", user_info.email);
    if let Some(ref a) = db.get_account(&user_info.email).await?.and_then(|acc| acc.alias) {
        println!("   Alias: {}", a);
    }
    println!("\nYou can now run the daemon to start syncing:");
    println!("  cargo run --bin groundeffect-daemon\n");

    Ok(())
}

/// Parse OAuth callback URL to extract code and state
fn parse_oauth_callback(request_line: &str) -> Result<(String, String)> {
    // Request line looks like: GET /oauth/callback?code=xxx&state=yyy HTTP/1.1
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("Invalid HTTP request");
    }

    let path = parts[1];
    if !path.starts_with("/oauth/callback") {
        anyhow::bail!("Unexpected callback path: {}", path);
    }

    // Parse query string
    let query_start = path.find('?').ok_or_else(|| anyhow::anyhow!("No query string"))?;
    let query = &path[query_start + 1..];

    let mut code = None;
    let mut state = None;

    for param in query.split('&') {
        let mut kv = param.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");
        match key {
            "code" => code = Some(urlencoding::decode(value)?.into_owned()),
            "state" => state = Some(urlencoding::decode(value)?.into_owned()),
            _ => {}
        }
    }

    let code = code.ok_or_else(|| anyhow::anyhow!("No authorization code in callback"))?;
    let state = state.ok_or_else(|| anyhow::anyhow!("No state in callback"))?;

    Ok((code, state))
}

/// List all configured accounts
async fn list_accounts() -> Result<()> {
    let config = Config::load().unwrap_or_default();

    if !config.lancedb_dir().exists() {
        println!("No accounts configured yet.");
        println!("\nAdd an account with:");
        println!("  cargo run --bin groundeffect-daemon -- add-account\n");
        return Ok(());
    }

    let db = Database::open(config.lancedb_dir()).await?;
    let accounts = db.list_accounts().await?;

    if accounts.is_empty() {
        println!("No accounts configured yet.");
        println!("\nAdd an account with:");
        println!("  cargo run --bin groundeffect-daemon -- add-account\n");
        return Ok(());
    }

    println!("\n📧 Configured Accounts:\n");
    for account in &accounts {
        let status_icon = match account.status {
            AccountStatus::Active => "✓",
            AccountStatus::NeedsReauth => "⚠",
            AccountStatus::Disabled => "○",
            AccountStatus::Syncing => "↻",
        };
        let alias_str = account
            .alias
            .as_ref()
            .map(|a| format!(" ({})", a))
            .unwrap_or_default();
        println!(
            "  {} {}{} - {:?}",
            status_icon, account.id, alias_str, account.status
        );

        let email_count = db.count_emails(Some(&account.id)).await.unwrap_or(0);
        let event_count = db.count_events(Some(&account.id)).await.unwrap_or(0);
        println!("    📨 {} emails, 📅 {} events", email_count, event_count);

        if let Some(last_sync) = account.last_sync_email {
            println!("    Last email sync: {}", last_sync.format("%Y-%m-%d %H:%M:%S"));
        }
    }
    println!();

    Ok(())
}

/// Remove an account
async fn remove_account(account: &str) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let db = Database::open(config.lancedb_dir()).await?;

    // Try to find account by email or alias
    let accounts = db.list_accounts().await?;
    let target = accounts.iter().find(|a| {
        a.id == account || a.alias.as_ref().map(|al| al == account).unwrap_or(false)
    });

    match target {
        Some(acc) => {
            println!("Removing account: {}", acc.id);

            // Remove from database
            db.delete_account(&acc.id).await?;

            // Remove tokens from keychain
            if let Err(e) = KeychainManager::delete_tokens(&acc.id) {
                warn!("Failed to remove tokens from keychain: {}", e);
            }

            println!("✅ Account removed successfully\n");
        }
        None => {
            println!("❌ Account not found: {}", account);
            println!("\nAvailable accounts:");
            for a in &accounts {
                println!("  - {}", a.id);
            }
            println!();
        }
    }

    Ok(())
}

/// Run the sync daemon
async fn run_daemon() -> Result<()> {
    info!("Starting GroundEffect daemon v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = Arc::new(Config::load().unwrap_or_else(|e| {
        warn!("Failed to load config: {}, using defaults", e);
        Config::default()
    }));

    // Ensure data directories exist
    std::fs::create_dir_all(config.lancedb_dir())?;
    std::fs::create_dir_all(config.attachments_dir())?;
    std::fs::create_dir_all(config.models_dir())?;
    std::fs::create_dir_all(config.sync_state_dir())?;

    // Initialize database
    info!("Opening database at {:?}", config.lancedb_dir());
    let db = Arc::new(Database::open(config.lancedb_dir()).await?);

    // Ensure indexes exist in background (doesn't block startup)
    let db_for_indexes = db.clone();
    tokio::spawn(async move {
        if let Err(e) = db_for_indexes.ensure_indexes().await {
            error!("Failed to ensure indexes: {}", e);
        }
    });

    // Check for accounts (warn but don't exit - accounts can be added via MCP)
    let accounts = db.list_accounts().await?;
    if accounts.is_empty() {
        info!("No accounts configured yet. Add accounts via MCP add_account tool.");
    }

    // Initialize embedding engine
    info!("Loading embedding model...");
    let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
        .unwrap_or(EmbeddingModel::MiniLML6);  // MiniLM works with Candle's BertConfig
    let embedding = Arc::new(
        EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_metal)
            .map_err(|e| {
                error!("Failed to load embedding model: {}", e);
                e
            })?,
    );

    // Initialize OAuth manager
    let oauth = Arc::new(OAuthManager::new());

    // Initialize sync manager
    let sync_manager = Arc::new(SyncManager::new(
        db.clone(),
        config.clone(),
        oauth.clone(),
        embedding.clone(),
    ));

    // Take the event receiver
    let mut event_rx = sync_manager
        .take_event_receiver()
        .expect("Event receiver already taken");

    // Load existing accounts and initialize sync
    info!("Found {} configured accounts", accounts.len());

    for account in &accounts {
        match sync_manager.init_account(account).await {
            Ok(_) => {
                info!("Initialized sync for account {}", account.id);

                // Always run initial_sync - it will decide whether to:
                // - Skip (if historical sync already complete)
                // - Resume (if partially synced)
                // - Start fresh (if no emails)
                match sync_manager.initial_sync(&account.id).await {
                    Ok(_) => info!("Sync check completed for {}", account.id),
                    Err(e) => error!("Sync failed for {}: {}", account.id, e),
                }

                // Start IMAP IDLE for real-time notifications
                if config.sync.email_idle_enabled {
                    if let Err(e) = sync_manager.start_idle(&account.id).await {
                        warn!("Failed to start IDLE for {}: {}", account.id, e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to initialize sync for {}: {}", account.id, e);
            }
        }
    }

    // Spawn event handler
    let sync_manager_clone = sync_manager.clone();
    let db_clone = db.clone();
    tokio::spawn(async move {
        // Track last FTS index rebuild time (rebuild at most every 5 minutes)
        let mut last_fts_rebuild: Option<std::time::Instant> = None;
        let fts_rebuild_interval = std::time::Duration::from_secs(300);

        while let Some(event) = event_rx.recv().await {
            match event {
                SyncEvent::NewEmail { account_id, .. } => {
                    info!("New email notification for {}", account_id);
                    // Trigger incremental sync
                    if let Err(e) = sync_manager_clone
                        .trigger_sync(&[account_id.clone()], SyncType::Email)
                        .await
                    {
                        error!("Failed to sync after new email: {}", e);
                    }
                }
                SyncEvent::SyncCompleted {
                    account_id,
                    sync_type,
                    count,
                } => {
                    info!(
                        "Sync completed for {} ({:?}): {} items",
                        account_id, sync_type, count
                    );

                    // Rebuild FTS indexes if enough time has passed since last rebuild
                    if count > 0 {
                        let should_rebuild = last_fts_rebuild
                            .map(|t| t.elapsed() >= fts_rebuild_interval)
                            .unwrap_or(true);

                        if should_rebuild {
                            let db_for_fts = db_clone.clone();
                            tokio::spawn(async move {
                                if let Err(e) = db_for_fts.rebuild_fts_indexes().await {
                                    error!("Failed to rebuild FTS indexes: {}", e);
                                }
                            });
                            last_fts_rebuild = Some(std::time::Instant::now());
                        }
                    }
                }
                SyncEvent::SyncError { account_id, error } => {
                    error!("Sync error for {}: {}", account_id, error);
                }
                SyncEvent::AuthRequired { account_id } => {
                    warn!("Re-authentication required for {}", account_id);
                    // Update account status in database
                    if let Ok(Some(mut account)) = db_clone.get_account(&account_id).await {
                        account.status = AccountStatus::NeedsReauth;
                        if let Err(e) = db_clone.upsert_account(&account).await {
                            error!("Failed to update account status: {}", e);
                        }
                    }
                }
                _ => {
                    info!("Sync event: {:?}", event);
                }
            }
        }
    });

    // Track initialized accounts
    let initialized_accounts: Arc<RwLock<std::collections::HashSet<String>>> =
        Arc::new(RwLock::new(accounts.iter().map(|a| a.id.clone()).collect()));

    // Spawn periodic sync task with new account detection
    let sync_manager_poll = sync_manager.clone();
    let config_poll = config.clone();
    let db_poll = db.clone();
    let initialized_accounts_poll = initialized_accounts.clone();
    tokio::spawn(async move {
        let email_interval =
            tokio::time::Duration::from_secs(config_poll.sync.email_poll_interval_secs);
        let calendar_interval =
            tokio::time::Duration::from_secs(config_poll.sync.calendar_poll_interval_secs);
        // Check for new accounts every 5 seconds
        let new_account_interval = tokio::time::Duration::from_secs(5);

        let mut email_timer = tokio::time::interval(email_interval);
        let mut calendar_timer = tokio::time::interval(calendar_interval);
        let mut new_account_timer = tokio::time::interval(new_account_interval);

        loop {
            tokio::select! {
                _ = new_account_timer.tick() => {
                    // Check for newly added accounts
                    if let Ok(accounts) = db_poll.list_accounts().await {
                        for account in &accounts {
                            let is_new = !initialized_accounts_poll.read().unwrap().contains(&account.id);
                            if is_new {
                                info!("Detected new account: {}", account.id);
                                initialized_accounts_poll.write().unwrap().insert(account.id.clone());

                                // Initialize sync for the new account
                                match sync_manager_poll.init_account(account).await {
                                    Ok(_) => {
                                        info!("Initialized sync for new account {}", account.id);

                                        // Run initial sync
                                        info!("Starting initial sync for {}", account.id);
                                        match sync_manager_poll.initial_sync(&account.id).await {
                                            Ok(_) => info!("Initial sync completed for {}", account.id),
                                            Err(e) => error!("Initial sync failed for {}: {}", account.id, e),
                                        }

                                        // Start IMAP IDLE
                                        if config_poll.sync.email_idle_enabled {
                                            if let Err(e) = sync_manager_poll.start_idle(&account.id).await {
                                                warn!("Failed to start IDLE for {}: {}", account.id, e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to initialize sync for new account {}: {}", account.id, e);
                                    }
                                }
                            }
                        }
                    }
                }
                _ = email_timer.tick() => {
                    // Poll sync for all accounts (fallback if IDLE is disabled or disconnected)
                    if let Ok(accounts) = db_poll.list_accounts().await {
                        let account_ids: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
                        if let Err(e) = sync_manager_poll.trigger_sync(&account_ids, SyncType::Email).await {
                            warn!("Periodic email sync failed: {}", e);
                        }
                    }
                }
                _ = calendar_timer.tick() => {
                    // Poll calendar sync for all accounts
                    if let Ok(accounts) = db_poll.list_accounts().await {
                        let account_ids: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
                        if let Err(e) = sync_manager_poll.trigger_sync(&account_ids, SyncType::Calendar).await {
                            warn!("Periodic calendar sync failed: {}", e);
                        }
                    }
                }
            }
        }
    });

    info!("Daemon is running. Press Ctrl+C to stop.");

    // Wait for shutdown signal
    signal::ctrl_c().await?;

    info!("Shutting down daemon...");
    Ok(())
}

/// Run the MCP server on stdio for Claude Code integration
async fn run_mcp_server() -> Result<()> {
    // Disable tracing output for MCP mode (it would interfere with stdio JSON-RPC)
    // We rely on the server's internal logging to stderr if needed

    // Load configuration
    let config = Arc::new(Config::load().unwrap_or_default());

    // Ensure data directories exist
    std::fs::create_dir_all(config.lancedb_dir())?;
    std::fs::create_dir_all(config.models_dir())?;

    // Initialize database
    let db = Arc::new(Database::open(config.lancedb_dir()).await?);

    // Initialize embedding engine
    let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
        .unwrap_or(EmbeddingModel::MiniLML6);
    let embedding = Arc::new(
        EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_metal)?,
    );

    // Initialize OAuth manager
    let oauth = Arc::new(OAuthManager::new());

    // Create and run MCP server
    let mcp = McpServer::new(db, config, embedding, oauth);
    mcp.run().await.map_err(|e| anyhow::anyhow!(e))
}

/// Run the setup wizard
fn run_setup(install: bool, uninstall: bool) -> Result<()> {
    if uninstall {
        return uninstall_launchd_agent();
    }

    println!("\n🔧 GroundEffect Setup Wizard\n");

    // Load or create daemon config
    let mut daemon_config = DaemonConfig::load().unwrap_or_default();

    // Interactive configuration
    println!("Configure daemon settings (press Enter for defaults):");
    println!("(These can be changed later with `groundeffect-daemon configure`)\n");

    // Logging
    let logging_enabled = Confirm::new()
        .with_prompt("Enable logging to ~/.local/share/groundeffect/logs/")
        .default(daemon_config.logging_enabled)
        .interact()?;
    daemon_config.logging_enabled = logging_enabled;

    // Email poll interval
    let email_interval: u64 = Input::new()
        .with_prompt("Email poll interval (seconds)")
        .default(daemon_config.email_poll_interval_secs)
        .interact_text()?;
    daemon_config.email_poll_interval_secs = email_interval;

    // Calendar poll interval
    let calendar_interval: u64 = Input::new()
        .with_prompt("Calendar poll interval (seconds)")
        .default(daemon_config.calendar_poll_interval_secs)
        .interact_text()?;
    daemon_config.calendar_poll_interval_secs = calendar_interval;

    // Max concurrent fetches
    let max_fetches: usize = Input::new()
        .with_prompt("Max concurrent email fetches")
        .default(daemon_config.max_concurrent_fetches)
        .interact_text()?;
    daemon_config.max_concurrent_fetches = max_fetches;

    // Save config
    daemon_config.save()?;
    println!("\n✅ Configuration saved to {:?}", DaemonConfig::config_path());

    // Install launchd agent if requested or prompt
    let should_install = if install {
        true
    } else {
        Confirm::new()
            .with_prompt("Install launchd agent for auto-start at login?")
            .default(true)
            .interact()?
    };

    if should_install {
        install_launchd_agent(&daemon_config)?;
    } else {
        println!("\nTo start the daemon manually, run:");
        println!("  groundeffect-daemon run\n");
    }

    Ok(())
}

/// Run the configure command (interactive settings change)
fn run_configure() -> Result<()> {
    println!("\n⚙️  GroundEffect Configuration\n");

    // Load existing config
    let mut daemon_config = DaemonConfig::load().unwrap_or_default();

    println!("Current settings:");
    println!("  Logging: {}", if daemon_config.logging_enabled { "enabled" } else { "disabled" });
    println!("  Email poll interval: {}s", daemon_config.email_poll_interval_secs);
    println!("  Calendar poll interval: {}s", daemon_config.calendar_poll_interval_secs);
    println!("  Max concurrent fetches: {}", daemon_config.max_concurrent_fetches);
    println!();

    // Select what to change
    let options = &[
        "Logging",
        "Email poll interval",
        "Calendar poll interval",
        "Max concurrent fetches",
        "Change all settings",
        "Exit (no changes)",
    ];

    let selection = Select::new()
        .with_prompt("What would you like to change?")
        .items(options)
        .default(5)
        .interact()?;

    let changed = match selection {
        0 => {
            daemon_config.logging_enabled = Confirm::new()
                .with_prompt("Enable logging to ~/.local/share/groundeffect/logs/")
                .default(daemon_config.logging_enabled)
                .interact()?;
            true
        }
        1 => {
            daemon_config.email_poll_interval_secs = Input::new()
                .with_prompt("Email poll interval (seconds)")
                .default(daemon_config.email_poll_interval_secs)
                .interact_text()?;
            true
        }
        2 => {
            daemon_config.calendar_poll_interval_secs = Input::new()
                .with_prompt("Calendar poll interval (seconds)")
                .default(daemon_config.calendar_poll_interval_secs)
                .interact_text()?;
            true
        }
        3 => {
            daemon_config.max_concurrent_fetches = Input::new()
                .with_prompt("Max concurrent fetches")
                .default(daemon_config.max_concurrent_fetches)
                .interact_text()?;
            true
        }
        4 => {
            // Change all settings
            daemon_config.logging_enabled = Confirm::new()
                .with_prompt("Enable logging to ~/.local/share/groundeffect/logs/")
                .default(daemon_config.logging_enabled)
                .interact()?;
            daemon_config.email_poll_interval_secs = Input::new()
                .with_prompt("Email poll interval (seconds)")
                .default(daemon_config.email_poll_interval_secs)
                .interact_text()?;
            daemon_config.calendar_poll_interval_secs = Input::new()
                .with_prompt("Calendar poll interval (seconds)")
                .default(daemon_config.calendar_poll_interval_secs)
                .interact_text()?;
            daemon_config.max_concurrent_fetches = Input::new()
                .with_prompt("Max concurrent fetches")
                .default(daemon_config.max_concurrent_fetches)
                .interact_text()?;
            true
        }
        _ => {
            println!("No changes made.");
            false
        }
    };

    if changed {
        daemon_config.save()?;
        println!("\n✅ Configuration saved.");

        // Check if launchd agent is installed and offer to restart
        if DaemonConfig::is_launchd_installed() {
            let restart = Confirm::new()
                .with_prompt("Launchd agent detected. Restart daemon with new settings?")
                .default(true)
                .interact()?;

            if restart {
                restart_launchd_daemon()?;
            }
        }
    }

    Ok(())
}

/// Install the launchd agent
fn install_launchd_agent(config: &DaemonConfig) -> Result<()> {
    let plist_path = DaemonConfig::launchd_plist_path();

    // Find the daemon binary path
    let daemon_path = find_daemon_binary()?;

    // Create the LaunchAgents directory if it doesn't exist
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Generate the plist content
    let log_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".local")
        .join("share")
        .join("groundeffect")
        .join("logs");
    std::fs::create_dir_all(&log_dir)?;

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
        logging_flag = if config.logging_enabled { " --log" } else { "" },
        stdout = log_dir.join("stdout.log").display(),
        stderr = log_dir.join("stderr.log").display(),
        email_interval = config.email_poll_interval_secs,
        calendar_interval = config.calendar_poll_interval_secs,
        max_fetches = config.max_concurrent_fetches,
    );

    std::fs::write(&plist_path, plist_content)?;
    println!("✅ Created launchd plist at {:?}", plist_path);

    // Load the launchd agent
    let output = Command::new("launchctl")
        .args(["load", "-w", plist_path.to_str().unwrap()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("service already loaded") {
            anyhow::bail!("Failed to load launchd agent: {}", stderr);
        }
    }

    println!("✅ Launchd agent installed and started");
    println!("\nThe daemon will now start automatically at login.");
    println!("To check status: launchctl list | grep groundeffect");
    println!("To view logs: tail -f {:?}\n", log_dir.join("daemon.log"));

    Ok(())
}

/// Uninstall the launchd agent
fn uninstall_launchd_agent() -> Result<()> {
    let plist_path = DaemonConfig::launchd_plist_path();

    if !plist_path.exists() {
        println!("Launchd agent is not installed.");
        return Ok(());
    }

    println!("🗑️  Uninstalling launchd agent...\n");

    // Unload the agent
    let output = Command::new("launchctl")
        .args(["unload", "-w", plist_path.to_str().unwrap()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "could not find service" errors
        if !stderr.contains("Could not find specified service") {
            eprintln!("Warning: Failed to unload agent: {}", stderr);
        }
    }

    // Remove the plist file
    std::fs::remove_file(&plist_path)?;

    println!("✅ Launchd agent uninstalled");
    println!("The daemon will no longer start automatically at login.\n");

    Ok(())
}

/// Restart the daemon via launchctl
fn restart_launchd_daemon() -> Result<()> {
    println!("🔄 Restarting daemon...");

    let plist_path = DaemonConfig::launchd_plist_path();

    // Unload
    let _ = Command::new("launchctl")
        .args(["unload", plist_path.to_str().unwrap()])
        .output();

    // Brief pause
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Reload
    let output = Command::new("launchctl")
        .args(["load", "-w", plist_path.to_str().unwrap()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("service already loaded") {
            anyhow::bail!("Failed to restart daemon: {}", stderr);
        }
    }

    println!("✅ Daemon restarted with new settings\n");
    Ok(())
}

/// Find the daemon binary path
fn find_daemon_binary() -> Result<std::path::PathBuf> {
    // Check if we're running from cargo (development)
    if let Ok(exe) = std::env::current_exe() {
        if exe.exists() {
            return Ok(exe);
        }
    }

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
    let output = Command::new("which")
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

/// URL decoding
mod urlencoding {
    pub fn decode(s: &str) -> Result<std::borrow::Cow<str>, std::string::FromUtf8Error> {
        let mut result = Vec::with_capacity(s.len());
        let mut chars = s.bytes();

        while let Some(b) = chars.next() {
            if b == b'%' {
                if let (Some(h1), Some(h2)) = (chars.next(), chars.next()) {
                    if let Ok(byte) = u8::from_str_radix(
                        &format!("{}{}", h1 as char, h2 as char),
                        16,
                    ) {
                        result.push(byte);
                        continue;
                    }
                }
                result.push(b'%');
            } else if b == b'+' {
                result.push(b' ');
            } else {
                result.push(b);
            }
        }

        String::from_utf8(result).map(std::borrow::Cow::Owned)
    }
}
