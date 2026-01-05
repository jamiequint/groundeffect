//! GroundEffect MCP Server
//!
//! CLI spawned by Claude Code for read-only access to emails and calendar events.
//! Communicates via stdio JSON-RPC.

use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use groundeffect_core::config::Config;
use groundeffect_core::db::Database;
use groundeffect_core::embedding::{EmbeddingEngine, EmbeddingModel};
use groundeffect_core::mcp::McpServer;
use groundeffect_core::oauth::OAuthManager;

#[tokio::main]
async fn main() -> Result<()> {
    // Load config first to get log path
    let config = Arc::new(Config::load().unwrap_or_else(|_| Config::default()));

    // Set up file logging with timestamps
    let log_dir = config.general.data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "mcp.log");

    // Create a file layer with timestamps
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f".to_string()))
        .with_ansi(false)
        .with_target(false);

    // Initialize the subscriber with file logging at INFO level
    tracing_subscriber::registry()
        .with(file_layer.with_filter(tracing_subscriber::filter::LevelFilter::INFO))
        .init();

    info!("GroundEffect MCP server starting");

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
