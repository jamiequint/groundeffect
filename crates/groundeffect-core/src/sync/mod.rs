//! Email and calendar sync engine
//!
//! Handles IMAP sync (with IMAP IDLE) for Gmail and CalDAV sync for Google Calendar.

mod imap;
mod caldav;
mod rate_limiter;

pub use imap::*;
pub use caldav::*;
pub use rate_limiter::*;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::embedding::EmbeddingEngine;
use crate::error::{Error, Result};
use crate::keychain::KeychainManager;
use crate::models::{Account, AccountStatus};
use crate::oauth::OAuthManager;

/// Sync event types
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// New email received
    NewEmail { account_id: String, email_id: String },
    /// Email updated (flags changed)
    EmailUpdated { account_id: String, email_id: String },
    /// Email deleted
    EmailDeleted { account_id: String, email_id: String },
    /// New calendar event
    NewEvent { account_id: String, event_id: String },
    /// Calendar event updated
    EventUpdated { account_id: String, event_id: String },
    /// Calendar event deleted
    EventDeleted { account_id: String, event_id: String },
    /// Sync started
    SyncStarted { account_id: String, sync_type: SyncType },
    /// Sync completed
    SyncCompleted { account_id: String, sync_type: SyncType, count: usize },
    /// Sync error
    SyncError { account_id: String, error: String },
    /// Account needs re-authentication
    AuthRequired { account_id: String },
}

/// Type of sync operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncType {
    Email,
    Calendar,
    All,
}

/// Sync state for an account
#[derive(Debug, Clone)]
pub struct AccountSyncState {
    pub account_id: String,
    pub is_syncing: bool,
    pub last_email_sync: Option<DateTime<Utc>>,
    pub last_calendar_sync: Option<DateTime<Utc>>,
    pub email_count: u64,
    pub event_count: u64,
    pub error: Option<String>,
}

/// Sync manager for all accounts
pub struct SyncManager {
    db: Arc<Database>,
    config: Arc<Config>,
    oauth: Arc<OAuthManager>,
    embedding: Arc<EmbeddingEngine>,
    rate_limiter: Arc<GlobalRateLimiter>,
    account_states: RwLock<HashMap<String, AccountSyncState>>,
    event_tx: mpsc::Sender<SyncEvent>,
    event_rx: RwLock<Option<mpsc::Receiver<SyncEvent>>>,
}

impl SyncManager {
    /// Create a new sync manager
    pub fn new(
        db: Arc<Database>,
        config: Arc<Config>,
        oauth: Arc<OAuthManager>,
        embedding: Arc<EmbeddingEngine>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);
        let rate_limit = config.sync.rate_limit_per_second;

        Self {
            db,
            config,
            oauth,
            embedding,
            rate_limiter: Arc::new(GlobalRateLimiter::new(rate_limit)),
            account_states: RwLock::new(HashMap::new()),
            event_tx: tx,
            event_rx: RwLock::new(Some(rx)),
        }
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_receiver(&self) -> Option<mpsc::Receiver<SyncEvent>> {
        self.event_rx.write().take()
    }

    /// Get sync state for an account
    pub fn get_state(&self, account_id: &str) -> Option<AccountSyncState> {
        self.account_states.read().get(account_id).cloned()
    }

    /// Get sync states for all accounts
    pub fn get_all_states(&self) -> Vec<AccountSyncState> {
        self.account_states.read().values().cloned().collect()
    }

    /// Initialize sync for an account
    pub async fn init_account(&self, account: &Account) -> Result<()> {
        info!("Initializing sync for account {}", account.id);

        // Check if we have valid tokens
        let tokens = KeychainManager::get_tokens(&account.id)?
            .ok_or_else(|| Error::TokenExpired { account: account.id.clone() })?;

        if tokens.is_expired() {
            // Try to refresh
            match self.oauth.refresh_token(&account.id).await {
                Ok(_) => info!("Refreshed tokens for {}", account.id),
                Err(e) => {
                    warn!("Token refresh failed for {}: {}", account.id, e);
                    self.emit_event(SyncEvent::AuthRequired {
                        account_id: account.id.clone(),
                    }).await;
                    return Err(e);
                }
            }
        }

        // Initialize state
        let state = AccountSyncState {
            account_id: account.id.clone(),
            is_syncing: false,
            last_email_sync: account.last_sync_email,
            last_calendar_sync: account.last_sync_calendar,
            email_count: self.db.count_emails(Some(&account.id)).await?,
            event_count: self.db.count_events(Some(&account.id)).await?,
            error: None,
        };

        self.account_states.write().insert(account.id.clone(), state);
        Ok(())
    }

    /// Run initial sync for an account (smart sync strategy)
    pub async fn initial_sync(&self, account_id: &str) -> Result<()> {
        info!("Starting initial sync for {}", account_id);

        // Phase 1: Sync recent emails (last 90 days) + unread/flagged
        self.emit_event(SyncEvent::SyncStarted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Email,
        }).await;

        let imap_client = ImapClient::new(
            account_id,
            self.oauth.clone(),
            self.rate_limiter.clone(),
        ).await?;

        // Smart initial sync: last 90 days + unread + flagged
        let since = Utc::now() - Duration::days(90);
        let emails = imap_client.fetch_recent_emails(since, 1000).await?;

        info!("Fetched {} recent emails for {}", emails.len(), account_id);

        // Generate embeddings and store
        for email in &emails {
            let mut email = email.clone();
            let text = email.searchable_text();
            email.embedding = Some(self.embedding.embed(&text)?);
            self.db.upsert_email(&email).await?;
        }

        self.emit_event(SyncEvent::SyncCompleted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Email,
            count: emails.len(),
        }).await;

        // Update state
        if let Some(state) = self.account_states.write().get_mut(account_id) {
            state.last_email_sync = Some(Utc::now());
            state.email_count += emails.len() as u64;
            state.is_syncing = false;
        }

        // Phase 2: Start calendar sync
        self.sync_calendar(account_id).await?;

        // Phase 3: Background backfill will be handled by the daemon
        info!("Initial sync completed for {}", account_id);
        Ok(())
    }

    /// Sync calendar events for an account
    pub async fn sync_calendar(&self, account_id: &str) -> Result<()> {
        info!("Syncing calendar for {}", account_id);

        self.emit_event(SyncEvent::SyncStarted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Calendar,
        }).await;

        let caldav_client = CalDavClient::new(
            account_id,
            self.oauth.clone(),
            self.rate_limiter.clone(),
        ).await?;

        let events = caldav_client.fetch_events(None).await?;

        info!("Fetched {} calendar events for {}", events.len(), account_id);

        // Generate embeddings and store
        for event in &events {
            let mut event = event.clone();
            let text = event.searchable_text();
            event.embedding = Some(self.embedding.embed(&text)?);
            self.db.upsert_event(&event).await?;
        }

        self.emit_event(SyncEvent::SyncCompleted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Calendar,
            count: events.len(),
        }).await;

        // Update state
        if let Some(state) = self.account_states.write().get_mut(account_id) {
            state.last_calendar_sync = Some(Utc::now());
            state.event_count = events.len() as u64;
        }

        Ok(())
    }

    /// Start IMAP IDLE for real-time email notifications
    pub async fn start_idle(&self, account_id: &str) -> Result<()> {
        if !self.config.sync.email_idle_enabled {
            debug!("IMAP IDLE disabled, using polling instead");
            return Ok(());
        }

        info!("Starting IMAP IDLE for {}", account_id);

        let imap_client = ImapClient::new(
            account_id,
            self.oauth.clone(),
            self.rate_limiter.clone(),
        ).await?;

        let event_tx = self.event_tx.clone();
        let account_id = account_id.to_string();

        tokio::spawn(async move {
            if let Err(e) = imap_client.start_idle(event_tx).await {
                error!("IMAP IDLE error for {}: {}", account_id, e);
            }
        });

        Ok(())
    }

    /// Force sync for specific accounts
    pub async fn trigger_sync(&self, account_ids: &[String], sync_type: SyncType) -> Result<()> {
        for account_id in account_ids {
            match sync_type {
                SyncType::Email => {
                    // Incremental email sync
                    let imap_client = ImapClient::new(
                        account_id,
                        self.oauth.clone(),
                        self.rate_limiter.clone(),
                    ).await?;

                    let state = self.get_state(account_id);
                    let since = state
                        .and_then(|s| s.last_email_sync)
                        .unwrap_or_else(|| Utc::now() - Duration::hours(1));

                    let emails = imap_client.fetch_recent_emails(since, 100).await?;

                    for email in &emails {
                        let mut email = email.clone();
                        let text = email.searchable_text();
                        email.embedding = Some(self.embedding.embed(&text)?);
                        self.db.upsert_email(&email).await?;
                    }
                }
                SyncType::Calendar => {
                    self.sync_calendar(account_id).await?;
                }
                SyncType::All => {
                    Box::pin(self.trigger_sync(&[account_id.clone()], SyncType::Email)).await?;
                    Box::pin(self.trigger_sync(&[account_id.clone()], SyncType::Calendar)).await?;
                }
            }
        }
        Ok(())
    }

    /// Emit a sync event
    async fn emit_event(&self, event: SyncEvent) {
        if let Err(e) = self.event_tx.send(event).await {
            warn!("Failed to send sync event: {}", e);
        }
    }
}
