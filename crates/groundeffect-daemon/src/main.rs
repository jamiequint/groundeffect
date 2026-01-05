//! GroundEffect Daemon
//!
//! Long-running launchd service that handles email/calendar sync,
//! indexing, and writes to LanceDB.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{error, info, warn, Level};
use tracing_subscriber::fmt::format::FmtSpan;

use groundeffect_core::config::Config;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging (but not for MCP mode - it uses stdio for JSON-RPC)
    let is_mcp = matches!(cli.command, Some(Commands::Mcp));
    if !is_mcp {
        let _subscriber = tracing_subscriber::fmt()
            .with_max_level(Level::INFO)
            .with_span_events(FmtSpan::CLOSE)
            .with_target(true)
            .with_thread_ids(true)
            .init();
    }

    match cli.command {
        Some(Commands::AddAccount { alias }) => add_account(alias).await,
        Some(Commands::ListAccounts) => list_accounts().await,
        Some(Commands::RemoveAccount { account }) => remove_account(&account).await,
        Some(Commands::Run) | None => run_daemon().await,
        Some(Commands::Mcp) => run_mcp_server().await,
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
Content-Type: text/html

<!DOCTYPE html>
<html>
<head><title>GroundEffect - Success</title></head>
<body style="font-family: -apple-system, BlinkMacSystemFont, sans-serif; padding: 40px; text-align: center;">
    <h1>✅ Authentication Successful!</h1>
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

    println!("✅ Tokens stored in macOS Keychain\n");

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
        // Create new account
        let account = Account {
            id: user_info.email.clone(),
            alias,
            display_name: user_info.name.unwrap_or_else(|| user_info.email.clone()),
            added_at: Utc::now(),
            last_sync_email: None,
            last_sync_calendar: None,
            status: AccountStatus::Active,
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

    // Check for accounts
    let accounts = db.list_accounts().await?;
    if accounts.is_empty() {
        println!("\n⚠️  No accounts configured!\n");
        println!("Add an account first:");
        println!("  cargo run --bin groundeffect-daemon -- add-account\n");
        return Ok(());
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

                // Check if we need to do initial sync (no emails yet)
                let email_count = db.count_emails(Some(&account.id)).await.unwrap_or(0);
                if email_count == 0 {
                    info!("No emails found, running initial sync for {}", account.id);
                    match sync_manager.initial_sync(&account.id).await {
                        Ok(_) => info!("Initial sync completed for {}", account.id),
                        Err(e) => error!("Initial sync failed for {}: {}", account.id, e),
                    }
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

    // Spawn periodic sync task
    let sync_manager_poll = sync_manager.clone();
    let config_poll = config.clone();
    let db_poll = db.clone();
    tokio::spawn(async move {
        let email_interval =
            tokio::time::Duration::from_secs(config_poll.sync.email_poll_interval_secs);
        let calendar_interval =
            tokio::time::Duration::from_secs(config_poll.sync.calendar_poll_interval_secs);

        let mut email_timer = tokio::time::interval(email_interval);
        let mut calendar_timer = tokio::time::interval(calendar_interval);

        loop {
            tokio::select! {
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
