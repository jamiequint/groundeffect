//! GroundEffect MCP Server
//!
//! CLI spawned by Claude Code for read-only access to emails and calendar events.
//! Communicates via stdio JSON-RPC.

use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info, Level};

use groundeffect_core::config::Config;
use groundeffect_core::db::Database;
use groundeffect_core::embedding::{EmbeddingEngine, EmbeddingModel};
use groundeffect_core::mcp::McpServer;
use groundeffect_core::oauth::OAuthManager;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to stderr (stdout is used for JSON-RPC)
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::WARN)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .init();

    // Load configuration
    let config = Arc::new(Config::load().unwrap_or_else(|_| Config::default()));

    // Check if database exists
    let db_path = config.lancedb_dir();
    if !db_path.exists() {
        error!("Database not found at {:?}", db_path);
        error!("Please run the GroundEffect daemon first to initialize the database.");
        std::process::exit(1);
    }

    // Open database in read-only mode
    // Note: LanceDB supports concurrent readers, so this is safe
    let db = Arc::new(Database::open(&db_path).await?);

    // Initialize embedding engine for search queries
    let model_type = EmbeddingModel::from_str(&config.search.embedding_model)
        .unwrap_or(EmbeddingModel::NomicEmbedText);
    let embedding = Arc::new(
        EmbeddingEngine::from_cache(config.models_dir(), model_type, config.search.use_metal)
            .map_err(|e| {
                error!("Failed to load embedding model: {}", e);
                e
            })?,
    );

    // Initialize OAuth manager (for mutations that go directly to IMAP/CalDAV)
    let oauth = Arc::new(OAuthManager::new());

    // Create and run MCP server
    let server = McpServer::new(db, config, embedding, oauth);

    info!("Starting MCP server on stdio");
    server.run().await?;

    Ok(())
}
