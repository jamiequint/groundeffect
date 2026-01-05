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
use crate::models::{Account, AccountStatus, CalendarEvent, Email};
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

use serde::{Deserialize, Serialize};

/// Sync state for an account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSyncState {
    pub account_id: String,
    pub is_syncing: bool,
    pub last_email_sync: Option<DateTime<Utc>>,
    pub last_calendar_sync: Option<DateTime<Utc>>,
    pub email_count: u64,
    pub event_count: u64,
    pub error: Option<String>,
    /// Initial sync progress (None if not currently doing initial sync)
    pub initial_sync_progress: Option<InitialSyncProgress>,
}

/// Progress tracking for initial sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitialSyncProgress {
    /// Total emails estimated on server
    pub total_emails_estimated: u64,
    /// Emails synced so far
    pub emails_synced: u64,
    /// Total events estimated
    pub total_events_estimated: u64,
    /// Events synced so far
    pub events_synced: u64,
    /// When sync started
    pub started_at: DateTime<Utc>,
    /// Current phase of sync
    pub phase: SyncPhase,
    /// Emails per second (smoothed)
    pub emails_per_second: f64,
}

impl InitialSyncProgress {
    /// Calculate percentage complete (0.0 - 100.0)
    pub fn percentage_complete(&self) -> f64 {
        let total = self.total_emails_estimated + self.total_events_estimated;
        if total == 0 {
            return 0.0;
        }
        let synced = self.emails_synced + self.events_synced;
        (synced as f64 / total as f64) * 100.0
    }

    /// Estimate seconds remaining
    pub fn estimated_seconds_remaining(&self) -> Option<u64> {
        if self.emails_per_second <= 0.0 {
            return None;
        }
        let remaining = (self.total_emails_estimated + self.total_events_estimated)
            .saturating_sub(self.emails_synced + self.events_synced);
        Some((remaining as f64 / self.emails_per_second) as u64)
    }
}

/// Phase of initial sync
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncPhase {
    /// Counting emails on server
    Counting,
    /// Syncing recent emails (last 90 days)
    RecentEmails,
    /// Syncing calendar events
    Calendar,
    /// Backfilling older emails
    Backfill,
    /// Completed
    Completed,
}

/// Sync manager for all accounts
pub struct SyncManager {
    db: Arc<Database>,
    config: Arc<Config>,
    oauth: Arc<OAuthManager>,
    embedding: Arc<EmbeddingEngine>,
    rate_limiter: Arc<GlobalRateLimiter>,
    account_states: Arc<RwLock<HashMap<String, AccountSyncState>>>,
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
            account_states: Arc::new(RwLock::new(HashMap::new())),
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
            initial_sync_progress: None,
        };

        self.account_states.write().insert(account.id.clone(), state);
        Ok(())
    }

    /// Run initial sync for an account (smart sync strategy - newest first)
    pub async fn initial_sync(&self, account_id: &str) -> Result<()> {
        info!("Starting initial sync for {}", account_id);

        let imap_client = ImapClient::new(
            account_id,
            self.oauth.clone(),
            self.rate_limiter.clone(),
        ).await?;

        // Phase 0: Count emails to get progress estimate
        {
            let mut states = self.account_states.write();
            if let Some(state) = states.get_mut(account_id) {
                state.initial_sync_progress = Some(InitialSyncProgress {
                    total_emails_estimated: 0,
                    emails_synced: 0,
                    total_events_estimated: 0,
                    events_synced: 0,
                    started_at: Utc::now(),
                    phase: SyncPhase::Counting,
                    emails_per_second: 0.0,
                });
            }
        }

        // Count total emails in INBOX (for progress estimation)
        let total_emails = imap_client.count_emails().await.unwrap_or(0);
        info!("Account {} has approximately {} emails in INBOX", account_id, total_emails);

        // Update progress with count
        {
            let mut states = self.account_states.write();
            if let Some(state) = states.get_mut(account_id) {
                if let Some(ref mut progress) = state.initial_sync_progress {
                    progress.total_emails_estimated = total_emails;
                    progress.phase = SyncPhase::RecentEmails;
                }
            }
        }

        // Phase 1: Sync recent emails - NEWEST FIRST (single IMAP connection)
        self.emit_event(SyncEvent::SyncStarted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Email,
        }).await;

        // Get account's sync_email_since preference, default to 90 days
        let account = self.db.get_account(account_id).await?
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;
        let since = account.sync_email_since.unwrap_or_else(|| Utc::now() - Duration::days(90));
        let batch_size = 50; // Fetch in batches for progress tracking
        let sync_start = std::time::Instant::now();

        // State for progress tracking (shared with callback)
        let total_synced = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let total_synced_clone = total_synced.clone();
        let account_states_clone = self.account_states.clone();
        let account_id_owned = account_id.to_string();
        let db_clone = self.db.clone();
        let embedding_clone = self.embedding.clone();

        // Use single connection to fetch all emails
        let result = imap_client.fetch_all_emails_since(since, batch_size, |emails| {
            let total_synced = total_synced_clone.clone();
            let account_states = account_states_clone.clone();
            let account_id = account_id_owned.clone();
            let db = db_clone.clone();
            let embedding = embedding_clone.clone();
            let sync_start = sync_start.clone();

            async move {
                let batch_count = emails.len();

                // Generate embeddings in batches for performance
                const EMBED_BATCH_SIZE: usize = 128;
                for embed_chunk in emails.chunks(EMBED_BATCH_SIZE) {
                    let texts: Vec<String> = embed_chunk.iter().map(|e| e.searchable_text()).collect();
                    let embeddings = embedding.embed_batch(&texts)?;

                    let emails_with_embeddings: Vec<Email> = embed_chunk
                        .iter()
                        .zip(embeddings.into_iter())
                        .map(|(email, embedding)| {
                            let mut email = email.clone();
                            email.embedding = Some(embedding);
                            email
                        })
                        .collect();

                    db.upsert_emails(&emails_with_embeddings).await?;
                }

                let prev = total_synced.fetch_add(batch_count, std::sync::atomic::Ordering::SeqCst);
                let new_total = prev + batch_count;
                info!("Embedded and stored {} emails (total: {}) for {}", batch_count, new_total, account_id);

                // Update progress
                {
                    let elapsed = sync_start.elapsed().as_secs_f64();
                    let mut states = account_states.write();
                    if let Some(state) = states.get_mut(&account_id) {
                        state.email_count = new_total as u64;
                        if let Some(ref mut progress) = state.initial_sync_progress {
                            progress.emails_synced = new_total as u64;
                            progress.emails_per_second = if elapsed > 0.0 {
                                new_total as f64 / elapsed
                            } else {
                                0.0
                            };
                        }
                    }
                }

                Ok(())
            }
        }).await;

        // Handle any error from the fetch operation
        if let Err(e) = result {
            error!("Error during email fetch for {}: {}", account_id, e);
            return Err(e);
        }

        let total_synced = total_synced.load(std::sync::atomic::Ordering::SeqCst);

        // Write progress file for MCP to read
        self.write_progress_file();

        info!("Email sync complete for {} - {} emails embedded and stored", account_id, total_synced);

        self.emit_event(SyncEvent::SyncCompleted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Email,
            count: total_synced,
        }).await;

        // Update state and persist to database
        let now = Utc::now();
        {
            let mut states = self.account_states.write();
            if let Some(state) = states.get_mut(account_id) {
                state.last_email_sync = Some(now);
                state.is_syncing = false;
                if let Some(ref mut progress) = state.initial_sync_progress {
                    progress.phase = SyncPhase::Calendar;
                }
            }
        }

        // Persist last_sync_email to database
        if let Ok(Some(mut account)) = self.db.get_account(account_id).await {
            account.last_sync_email = Some(now);
            if let Err(e) = self.db.upsert_account(&account).await {
                warn!("Failed to persist last_sync_email: {}", e);
            }
        }

        // Phase 2: Start calendar sync
        self.sync_calendar(account_id).await?;

        // Mark as completed
        {
            let mut states = self.account_states.write();
            if let Some(state) = states.get_mut(account_id) {
                state.is_syncing = false;
                if let Some(ref mut progress) = state.initial_sync_progress {
                    progress.phase = SyncPhase::Completed;
                }
            }
        }
        // Final progress update, then clear progress file
        self.write_progress_file();
        self.clear_progress_file();

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

        // Get account's sync_since setting
        let account = self.db.get_account(account_id).await?
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;
        let since = account.sync_email_since; // Uses same date range as email

        let caldav_client = CalDavClient::new(
            account_id,
            self.oauth.clone(),
            self.rate_limiter.clone(),
        ).await?;

        let events = caldav_client.fetch_events(since).await?;

        let fetched_count = events.len();
        info!("Fetched {} calendar events for {}", fetched_count, account_id);

        // Get existing event etags to skip unchanged events
        let existing_etags = self.db.get_event_etags(account_id).await.unwrap_or_default();

        // Filter to only new or changed events (compare by google_event_id and etag)
        let changed_events: Vec<_> = events
            .into_iter()
            .filter(|e| {
                match existing_etags.get(&e.google_event_id) {
                    Some(existing_etag) => existing_etag != &e.etag, // Changed
                    None => true, // New event
                }
            })
            .collect();
        let changed_count = changed_events.len();

        if changed_events.is_empty() {
            info!("Calendar sync complete for {} - no changes detected", account_id);
        } else {
            info!("{} new/changed calendar events to process for {}", changed_events.len(), account_id);

            // Generate embeddings in batches for performance
            // 128 is optimal for M4 Apple Silicon, use 64 for M1-M3
            const BATCH_SIZE: usize = 128;
            let total = changed_events.len();
            let mut processed = 0;

            for chunk in changed_events.chunks(BATCH_SIZE) {
                // Collect texts for batch embedding
                let texts: Vec<String> = chunk.iter().map(|e| e.searchable_text()).collect();

                // Generate embeddings for entire batch at once
                let embeddings = self.embedding.embed_batch(&texts)?;

                // Attach embeddings to events
                let events_with_embeddings: Vec<CalendarEvent> = chunk
                    .iter()
                    .zip(embeddings.into_iter())
                    .map(|(event, embedding)| {
                        let mut event = event.clone();
                        event.embedding = Some(embedding);
                        event
                    })
                    .collect();

                // Batch insert all events at once
                self.db.upsert_events(&events_with_embeddings).await?;

                processed += chunk.len();

                // Log progress every batch
                info!("Embedded and stored {}/{} calendar events for {}", processed, total, account_id);
            }

            info!("Calendar sync complete for {} - {} events updated", account_id, total);
        }

        self.emit_event(SyncEvent::SyncCompleted {
            account_id: account_id.to_string(),
            sync_type: SyncType::Calendar,
            count: changed_count,
        }).await;

        // Update state and persist to database
        let now = Utc::now();
        let total_event_count = self.db.count_events(Some(account_id)).await.unwrap_or(0);
        if let Some(state) = self.account_states.write().get_mut(account_id) {
            state.last_calendar_sync = Some(now);
            state.event_count = total_event_count;
        }

        // Persist last_sync_calendar to database
        if let Ok(Some(mut account)) = self.db.get_account(account_id).await {
            account.last_sync_calendar = Some(now);
            if let Err(e) = self.db.upsert_account(&account).await {
                warn!("Failed to persist last_sync_calendar: {}", e);
            }
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
                    debug!("Starting incremental email sync for {}", account_id);
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

                    if !emails.is_empty() {
                        info!("Incremental sync: found {} new emails for {}", emails.len(), account_id);

                        // Batch embed and insert for performance
                        const EMBED_BATCH_SIZE: usize = 128;
                        for chunk in emails.chunks(EMBED_BATCH_SIZE) {
                            let texts: Vec<String> = chunk.iter().map(|e| e.searchable_text()).collect();
                            let embeddings = self.embedding.embed_batch(&texts)?;

                            let emails_with_embeddings: Vec<Email> = chunk
                                .iter()
                                .zip(embeddings.into_iter())
                                .map(|(email, embedding)| {
                                    let mut email = email.clone();
                                    email.embedding = Some(embedding);
                                    email
                                })
                                .collect();

                            self.db.upsert_emails(&emails_with_embeddings).await?;
                        }
                        info!("Incremental sync: stored {} emails for {}", emails.len(), account_id);
                    }

                    // Update state and persist to database
                    let now = Utc::now();
                    if let Some(state) = self.account_states.write().get_mut(account_id) {
                        state.last_email_sync = Some(now);
                    }
                    if let Ok(Some(mut account)) = self.db.get_account(account_id).await {
                        account.last_sync_email = Some(now);
                        if let Err(e) = self.db.upsert_account(&account).await {
                            warn!("Failed to persist last_sync_email: {}", e);
                        }
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

    /// Write current sync progress to file for MCP to read
    fn write_progress_file(&self) {
        let states: Vec<AccountSyncState> = self.account_states.read().values().cloned().collect();
        let progress_file = self.config.sync_progress_file();

        match serde_json::to_string_pretty(&states) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&progress_file, json) {
                    debug!("Failed to write progress file: {}", e);
                }
            }
            Err(e) => {
                debug!("Failed to serialize progress: {}", e);
            }
        }
    }

    /// Clear progress file when sync is complete
    fn clear_progress_file(&self) {
        let progress_file = self.config.sync_progress_file();
        let _ = std::fs::remove_file(&progress_file);
    }
}
