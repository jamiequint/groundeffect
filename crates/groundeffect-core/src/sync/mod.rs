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

    /// Run initial sync for an account (smart sync strategy - newest first, with resume support)
    pub async fn initial_sync(&self, account_id: &str) -> Result<()> {
        info!("Starting initial sync for {}", account_id);

        // Mark as syncing
        {
            let mut states = self.account_states.write();
            if let Some(state) = states.get_mut(account_id) {
                state.is_syncing = true;
            }
        }

        // Get account's sync_email_since preference, default to 90 days
        let account = self.db.get_account(account_id).await?
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;
        let target_since = account.sync_email_since.unwrap_or_else(|| Utc::now() - Duration::days(90));

        // Check for existing emails to enable resume
        let (oldest_synced, _newest_synced) = self.db.get_email_sync_boundaries(account_id).await?;
        let existing_count = self.db.count_emails(Some(account_id)).await?;

        // Determine sync strategy:
        // 1. If backfill complete (oldest_synced date <= target_since date): incremental from last_sync_email
        // 2. If backfill incomplete: continue from target_since (with deduplication)
        // 3. Fresh sync: start from now, work back to target_since
        // Compare dates only (not timestamps) to handle emails that arrived after midnight on target day
        let backfill_complete = oldest_synced
            .map(|o| o.date_naive() <= target_since.date_naive())
            .unwrap_or(false);

        // fetch_before: Some(date) for backfill mode to limit search range, None for incremental
        let (resume_mode, fetch_since, fetch_before) = if backfill_complete {
            // Backfill complete - only need incremental sync for new emails
            let incremental_since = account.last_sync_email
                .map(|t| t - Duration::hours(1)) // 1 hour buffer for safety
                .unwrap_or(target_since);
            info!(
                "Incremental sync for {} from {} (backfill complete, {} emails)",
                account_id,
                incremental_since.format("%Y-%m-%d %H:%M"),
                existing_count
            );
            (false, incremental_since, None)
        } else if let Some(oldest) = oldest_synced {
            // Backfill incomplete - only fetch the missing date range
            // Add 2-day buffer on both ends to catch boundary/timezone edge cases
            // (IMAP search will also add buffer to the before date)
            let since_with_buffer = target_since - Duration::days(2);
            info!(
                "Continuing backfill for {} from {} to {} (had {} emails)",
                account_id,
                since_with_buffer.format("%Y-%m-%d"),
                oldest.format("%Y-%m-%d"),
                existing_count
            );
            (true, since_with_buffer, Some(oldest))
        } else {
            // Fresh sync - fetch all emails back to target_since
            info!("Fresh sync for {} back to {}", account_id, target_since.format("%Y-%m-%d"));
            (false, target_since, None)
        };

        // Skip email sync if very recent (within last 5 minutes) and backfill complete
        let skip_email_sync = backfill_complete && account.last_sync_email
            .map(|t| Utc::now() - t < Duration::minutes(5))
            .unwrap_or(false);

        if skip_email_sync {
            info!("Email sync recently completed for {}, proceeding to calendar", account_id);
        } else {
            // Load existing message_ids for deduplication
            let existing_message_ids = std::sync::Arc::new(
                self.db.get_email_message_ids(account_id).await.unwrap_or_default()
            );
            info!("Loaded {} existing message_ids for deduplication", existing_message_ids.len());

            let imap_client = ImapClient::new(
                account_id,
                self.oauth.clone(),
                self.rate_limiter.clone(),
            ).await?;

            // Phase 0: Count emails to get progress estimate
            // For incremental sync (backfill_complete), we don't need total INBOX count
            let total_emails = if backfill_complete {
                // Incremental sync - total is just existing + any new we find
                existing_count
            } else {
                // Backfill sync - count emails since target date for accurate progress
                let count = imap_client.count_emails_since(target_since).await.unwrap_or(0);
                info!("Account {} has approximately {} emails since {}", account_id, count, target_since.format("%Y-%m-%d"));

                // Persist estimated total to database for CLI status
                let mut updated_account = account.clone();
                updated_account.estimated_total_emails = Some(count);
                if let Err(e) = self.db.upsert_account(&updated_account).await {
                    warn!("Failed to persist estimated_total_emails: {}", e);
                }

                count
            };

            {
                let mut states = self.account_states.write();
                if let Some(state) = states.get_mut(account_id) {
                    let phase = if backfill_complete {
                        SyncPhase::RecentEmails
                    } else if resume_mode {
                        SyncPhase::Backfill
                    } else {
                        SyncPhase::Counting
                    };
                    state.initial_sync_progress = Some(InitialSyncProgress {
                        total_emails_estimated: total_emails,
                        emails_synced: existing_count,
                        total_events_estimated: 0,
                        events_synced: 0,
                        started_at: Utc::now(),
                        phase,
                        emails_per_second: 0.0,
                    });
                }
            }

            // Phase 1: Sync emails (either fresh, resuming, or incremental)
            self.emit_event(SyncEvent::SyncStarted {
                account_id: account_id.to_string(),
                sync_type: SyncType::Email,
            }).await;

            let batch_size = 256; // Fetch in batches (aligns with embedding batch size of 128)
            let sync_start = std::time::Instant::now();

            // State for progress tracking (shared with callback)
            let total_synced = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(existing_count as usize));
            let total_new = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let total_synced_clone = total_synced.clone();
            let total_new_clone = total_new.clone();
            let account_states_clone = self.account_states.clone();
            let account_id_owned = account_id.to_string();
            let db_clone = self.db.clone();
            let embedding_clone = self.embedding.clone();
            let existing_ids_clone = existing_message_ids.clone();
            let progress_file_path = self.config.sync_progress_file();

            // Use single connection to fetch emails (incremental or backfill depending on fetch_since)
            // fetch_before limits the date range during backfill to avoid re-fetching all emails
            let result = imap_client.fetch_all_emails_since(fetch_since, fetch_before, batch_size, |emails| {
                let total_synced = total_synced_clone.clone();
                let total_new = total_new_clone.clone();
                let account_states = account_states_clone.clone();
                let account_id = account_id_owned.clone();
                let db = db_clone.clone();
                let embedding = embedding_clone.clone();
                let sync_start = sync_start.clone();
                let existing_ids = existing_ids_clone.clone();
                let progress_path = progress_file_path.clone();

                async move {
                    // Filter out emails we already have (by message_id)
                    let new_emails: Vec<_> = emails
                        .into_iter()
                        .filter(|e| !existing_ids.contains(&e.message_id))
                        .collect();

                    if new_emails.is_empty() {
                        debug!("Batch contained only already-synced emails, skipping");
                        return Ok(());
                    }

                    let batch_count = new_emails.len();
                    let mut successfully_stored = 0;

                    // Generate embeddings in batches for performance
                    let embed_batch_size = self.config.search.embedding_batch_size;
                    const MAX_EMBED_RETRIES: u32 = 3;

                    for embed_chunk in new_emails.chunks(embed_batch_size) {
                        let texts: Vec<String> = embed_chunk.iter().map(|e| e.searchable_text()).collect();

                        // Retry embedding with exponential backoff
                        let mut embeddings_result = None;
                        let mut last_error = None;
                        for attempt in 1..=MAX_EMBED_RETRIES {
                            match embedding.embed_batch(&texts) {
                                Ok(emb) => {
                                    embeddings_result = Some(emb);
                                    break;
                                }
                                Err(e) => {
                                    warn!("Embedding attempt {}/{} failed: {}", attempt, MAX_EMBED_RETRIES, e);
                                    last_error = Some(e);
                                    if attempt < MAX_EMBED_RETRIES {
                                        let delay = 1000 * (1 << (attempt - 1)); // 1s, 2s, 4s
                                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                                    }
                                }
                            }
                        }

                        let embeddings = match embeddings_result {
                            Some(emb) => emb,
                            None => {
                                error!("Failed to embed batch after {} retries, skipping {} emails", MAX_EMBED_RETRIES, embed_chunk.len());
                                continue; // Skip this chunk but continue with others
                            }
                        };

                        let emails_with_embeddings: Vec<Email> = embed_chunk
                            .iter()
                            .zip(embeddings.into_iter())
                            .map(|(email, embedding)| {
                                let mut email = email.clone();
                                email.embedding = Some(embedding);
                                email
                            })
                            .collect();

                        // Retry database upsert with exponential backoff
                        let mut db_success = false;
                        for attempt in 1..=MAX_EMBED_RETRIES {
                            match db.upsert_emails(&emails_with_embeddings).await {
                                Ok(_) => {
                                    db_success = true;
                                    successfully_stored += emails_with_embeddings.len();
                                    break;
                                }
                                Err(e) => {
                                    warn!("DB upsert attempt {}/{} failed: {}", attempt, MAX_EMBED_RETRIES, e);
                                    if attempt < MAX_EMBED_RETRIES {
                                        let delay = 1000 * (1 << (attempt - 1));
                                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                                    }
                                }
                            }
                        }

                        if !db_success {
                            error!("Failed to store batch after {} retries, {} emails may be lost", MAX_EMBED_RETRIES, emails_with_embeddings.len());
                        }
                    }

                    if successfully_stored == 0 && batch_count > 0 {
                        return Err(Error::Sync(format!("Failed to store any emails from batch of {}", batch_count)));
                    }

                    let prev = total_synced.fetch_add(successfully_stored, std::sync::atomic::Ordering::SeqCst);
                    let new_total = prev + successfully_stored;
                    total_new.fetch_add(successfully_stored, std::sync::atomic::Ordering::SeqCst);

                    if successfully_stored < batch_count {
                        warn!("Stored {}/{} emails for {} (some failed)", successfully_stored, batch_count, account_id);
                    } else {
                        info!("Embedded and stored {} new emails (total: {}) for {}", successfully_stored, new_total, account_id);
                    }

                    // Update progress and write to file for MCP to read
                    {
                        let elapsed = sync_start.elapsed().as_secs_f64();
                        let states_snapshot: Vec<AccountSyncState>;
                        {
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
                            states_snapshot = states.values().cloned().collect();
                        }

                        // Write progress file for MCP to read live status
                        if let Ok(json) = serde_json::to_string_pretty(&states_snapshot) {
                            let _ = std::fs::write(&progress_path, json);
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

            let total_synced_count = total_synced.load(std::sync::atomic::Ordering::SeqCst);
            let new_emails_count = total_new.load(std::sync::atomic::Ordering::SeqCst);

            // Write progress file for MCP to read
            self.write_progress_file();

            info!(
                "Email sync complete for {} - {} new emails embedded (total: {})",
                account_id, new_emails_count, total_synced_count
            );

            self.emit_event(SyncEvent::SyncCompleted {
                account_id: account_id.to_string(),
                sync_type: SyncType::Email,
                count: new_emails_count,
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

            // Persist last_sync_email and oldest_email_synced to database
            let should_sync_attachments;
            if let Ok(Some(mut account)) = self.db.get_account(account_id).await {
                should_sync_attachments = account.sync_attachments;
                account.last_sync_email = Some(now);
                // Update oldest_email_synced to track sync progress for resume
                let (new_oldest, _) = self.db.get_email_sync_boundaries(account_id).await.unwrap_or((None, None));
                if new_oldest.is_some() {
                    account.oldest_email_synced = new_oldest;
                }
                if let Err(e) = self.db.upsert_account(&account).await {
                    warn!("Failed to persist sync state: {}", e);
                }
            } else {
                should_sync_attachments = false;
            }

            // Download attachments if enabled for this account
            if should_sync_attachments {
                info!("Downloading attachments for {} (sync_attachments enabled)", account_id);
                match self.download_attachments_for_account(account_id).await {
                    Ok((count, size)) => {
                        if count > 0 {
                            info!("Downloaded {} attachments ({} bytes) for {}", count, size, account_id);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to download attachments for {}: {}", account_id, e);
                    }
                }
            }
        } // End of email sync block

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
            let batch_size = self.config.search.embedding_batch_size;
            let total = changed_events.len();
            let mut processed = 0;

            for chunk in changed_events.chunks(batch_size) {
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

        // Persist last_sync_calendar and oldest_event_synced to database
        if let Ok(Some(mut account)) = self.db.get_account(account_id).await {
            account.last_sync_calendar = Some(now);
            // Update oldest_event_synced to track sync progress
            let (oldest_event, _) = self.db.get_event_sync_boundaries(account_id).await.unwrap_or((None, None));
            if oldest_event.is_some() {
                account.oldest_event_synced = oldest_event;
            }
            if let Err(e) = self.db.upsert_account(&account).await {
                warn!("Failed to persist last_sync_calendar: {}", e);
            }
        }

        Ok(())
    }

    /// Download attachments for emails that have them but haven't been downloaded yet
    /// Returns (downloaded_count, total_size_bytes)
    pub async fn download_attachments_for_account(&self, account_id: &str) -> Result<(usize, u64)> {
        info!("Downloading attachments for {}", account_id);

        let attachments_dir = self.config.attachments_dir();
        std::fs::create_dir_all(&attachments_dir)?;

        // Get emails with attachments that haven't been downloaded
        let emails = self.db.get_emails_with_pending_attachments(account_id).await?;

        if emails.is_empty() {
            info!("No pending attachments to download for {}", account_id);
            return Ok((0, 0));
        }

        info!("Found {} emails with pending attachments for {}", emails.len(), account_id);

        let imap_client = ImapClient::new(
            account_id,
            self.oauth.clone(),
            self.rate_limiter.clone(),
        ).await?;

        let mut total_downloaded = 0usize;
        let mut total_size = 0u64;

        for email in emails {
            if email.attachments.is_empty() {
                continue;
            }

            // Download all attachments for this email
            match imap_client.download_all_attachments(email.uid, &attachments_dir).await {
                Ok(downloaded) => {
                    if downloaded.is_empty() {
                        continue;
                    }

                    // Update email with downloaded attachment info
                    let mut updated_email = email.clone();
                    for (idx, att) in updated_email.attachments.iter_mut().enumerate() {
                        // Match by index since we download in order
                        if let Some((_, path, _, size)) = downloaded.get(idx) {
                            att.local_path = Some(path.clone());
                            att.downloaded = true;
                            total_size += size;
                        }
                    }

                    total_downloaded += downloaded.len();

                    // Update email in database
                    if let Err(e) = self.db.upsert_emails(&[updated_email]).await {
                        warn!("Failed to update email with attachment paths: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Failed to download attachments for email {}: {}", email.id, e);
                }
            }
        }

        info!(
            "Downloaded {} attachments ({} bytes) for {}",
            total_downloaded, total_size, account_id
        );

        Ok((total_downloaded, total_size))
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
                        let embed_batch_size = self.config.search.embedding_batch_size;
                        for chunk in emails.chunks(embed_batch_size) {
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
                    let should_sync_attachments;
                    if let Ok(Some(mut account)) = self.db.get_account(account_id).await {
                        should_sync_attachments = account.sync_attachments;
                        account.last_sync_email = Some(now);
                        if let Err(e) = self.db.upsert_account(&account).await {
                            warn!("Failed to persist last_sync_email: {}", e);
                        }
                    } else {
                        should_sync_attachments = false;
                    }

                    // Download attachments if enabled for this account
                    if should_sync_attachments {
                        if let Err(e) = self.download_attachments_for_account(account_id).await {
                            warn!("Failed to download attachments during incremental sync: {}", e);
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
