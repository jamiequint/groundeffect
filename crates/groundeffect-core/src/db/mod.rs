//! LanceDB database module
//!
//! Handles storage, retrieval, and search for emails and calendar events.
//! Uses LanceDB's built-in BM25 full-text search and vector ANN search.

mod schema;

use std::path::Path;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use chrono::{DateTime, Utc};
use lancedb::index::scalar::FtsIndexBuilder;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, Table};
use parking_lot::RwLock;
use tracing::{debug, info};

use crate::error::{Error, Result};
use crate::models::{Account, CalendarEvent, Email};
use crate::EMBEDDING_DIMENSION;

pub use schema::*;

/// Database table names
pub const EMAILS_TABLE: &str = "emails";
pub const EVENTS_TABLE: &str = "events";
pub const ACCOUNTS_TABLE: &str = "accounts";

/// Date validation constants for sync boundary calculations.
/// Dates outside this range are ignored to prevent a single bad record from breaking sync.
const MIN_VALID_YEAR: i32 = 1970;
const MAX_VALID_YEAR_OFFSET: i32 = 30; // current_year + 30

/// Check if a date is within a reasonable range for sync boundary calculations.
/// This prevents malformed dates (e.g., year 9474) from affecting sync logic.
fn is_reasonable_date(dt: &DateTime<Utc>) -> bool {
    use chrono::Datelike;
    let max_year = Utc::now().year() + MAX_VALID_YEAR_OFFSET;
    dt.year() >= MIN_VALID_YEAR && dt.year() <= max_year
}

/// LanceDB database wrapper
pub struct Database {
    connection: Connection,
    emails: RwLock<Option<Table>>,
    events: RwLock<Option<Table>>,
    accounts: RwLock<Option<Table>>,
}

impl Database {
    /// Open or create a database at the given path
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        info!("Opening LanceDB at {:?}", path);

        // Create directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let connection = connect(path.to_string_lossy().as_ref())
            .execute()
            .await?;

        let db = Self {
            connection,
            emails: RwLock::new(None),
            events: RwLock::new(None),
            accounts: RwLock::new(None),
        };

        // Initialize tables
        db.init_tables().await?;

        Ok(db)
    }

    /// Initialize database tables
    async fn init_tables(&self) -> Result<()> {
        // Check existing tables
        let table_names = self.connection.table_names().execute().await?;
        debug!("Existing tables: {:?}", table_names);

        // Create emails table if it doesn't exist
        if !table_names.contains(&EMAILS_TABLE.to_string()) {
            info!("Creating emails table");
            let schema = email_schema();
            let batch = empty_email_batch(&schema);
            let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(schema.clone()));
            let table = self
                .connection
                .create_table(EMAILS_TABLE, Box::new(batches))
                .execute()
                .await?;

            // Create FTS indices for BM25 search (one per column - LanceDB doesn't support composite)
            table
                .create_index(&["subject"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await?;
            table
                .create_index(&["body_plain"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await?;

            // Create scalar index on id for fast lookups
            table
                .create_index(&["id"], Index::BTree(Default::default()))
                .execute()
                .await?;

            // Note: Vector index will be created lazily once we have data
            // LanceDB requires data to train the IVF index

            *self.emails.write() = Some(table);
        } else {
            let table = self.connection.open_table(EMAILS_TABLE).execute().await?;
            *self.emails.write() = Some(table);
        }

        // Create events table if it doesn't exist
        if !table_names.contains(&EVENTS_TABLE.to_string()) {
            info!("Creating events table");
            let schema = event_schema();
            let batch = empty_event_batch(&schema);
            let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(schema.clone()));
            let table = self
                .connection
                .create_table(EVENTS_TABLE, Box::new(batches))
                .execute()
                .await?;

            // Create FTS indices (one per column - LanceDB doesn't support composite)
            table
                .create_index(&["summary"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await?;
            table
                .create_index(&["description"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await?;

            // Create scalar index on id for fast lookups
            table
                .create_index(&["id"], Index::BTree(Default::default()))
                .execute()
                .await?;

            // Note: Vector index will be created lazily once we have data

            *self.events.write() = Some(table);
        } else {
            let table = self.connection.open_table(EVENTS_TABLE).execute().await?;
            *self.events.write() = Some(table);
        }

        // Create accounts table if it doesn't exist
        if !table_names.contains(&ACCOUNTS_TABLE.to_string()) {
            info!("Creating accounts table");
            let schema = account_schema();
            let batch = empty_account_batch(&schema);
            let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(schema.clone()));
            let table = self
                .connection
                .create_table(ACCOUNTS_TABLE, Box::new(batches))
                .execute()
                .await?;
            *self.accounts.write() = Some(table);
        } else {
            let table = self.connection.open_table(ACCOUNTS_TABLE).execute().await?;

            // Check if schema migration is needed
            let expected_schema = account_schema();
            let table_schema = table.schema().await?;

            // Compare field count - if table has fewer fields, we need to migrate
            if table_schema.fields.len() < expected_schema.fields.len() {
                info!("Accounts table schema is outdated ({} fields vs {} expected), migrating...",
                      table_schema.fields.len(), expected_schema.fields.len());

                // Read all existing accounts
                let results = table.query().execute().await?;
                let batches: Vec<RecordBatch> = results.try_collect().await?;
                let mut accounts = Vec::new();
                for batch in &batches {
                    for i in 0..batch.num_rows() {
                        // Use a lenient parser that handles missing columns
                        if let Ok(account) = batch_to_account_lenient(batch, i) {
                            accounts.push(account);
                        }
                    }
                }
                info!("Read {} accounts for migration", accounts.len());

                // Drop the old table
                self.connection.drop_table(ACCOUNTS_TABLE, &[]).await?;

                // Create new table with correct schema
                let schema = account_schema();
                let batch = empty_account_batch(&schema);
                let batches_iter = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(schema.clone()));
                let new_table = self
                    .connection
                    .create_table(ACCOUNTS_TABLE, Box::new(batches_iter))
                    .execute()
                    .await?;

                // Re-insert accounts with new schema
                if !accounts.is_empty() {
                    for account in &accounts {
                        let batch = account_to_batch(account)?;
                        let batches_iter = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(account_schema()));
                        new_table.add(Box::new(batches_iter)).execute().await?;
                    }
                    info!("Migrated {} accounts to new schema", accounts.len());
                }

                *self.accounts.write() = Some(new_table);
            } else {
                *self.accounts.write() = Some(table);
            }
        }

        info!("Database tables initialized");
        Ok(())
    }

    /// Refresh table handles to see latest data
    /// Call this before queries if data may have been written by another process
    pub async fn refresh_tables(&self) -> Result<()> {
        let table_names = self.connection.table_names().execute().await?;

        if table_names.contains(&EMAILS_TABLE.to_string()) {
            let table = self.connection.open_table(EMAILS_TABLE).execute().await?;
            *self.emails.write() = Some(table);
        }
        if table_names.contains(&EVENTS_TABLE.to_string()) {
            let table = self.connection.open_table(EVENTS_TABLE).execute().await?;
            *self.events.write() = Some(table);
        }
        if table_names.contains(&ACCOUNTS_TABLE.to_string()) {
            let table = self.connection.open_table(ACCOUNTS_TABLE).execute().await?;
            *self.accounts.write() = Some(table);
        }

        debug!("Refreshed table handles");
        Ok(())
    }

    /// Ensure all indexes exist on tables
    /// This should be called by the daemon after startup, not by read-only clients.
    /// Indexes are created when tables are first created, but this handles upgrades
    /// from older databases that may be missing indexes.
    pub async fn ensure_indexes(&self) -> Result<()> {
        // Emails table indexes
        if let Ok(table) = self.emails_table() {
            let existing_indices = table.list_indices().await.unwrap_or_default();
            let existing_columns: std::collections::HashSet<_> = existing_indices
                .iter()
                .flat_map(|idx| idx.columns.clone())
                .collect();

            if !existing_columns.contains("subject") {
                info!("Creating FTS index on emails.subject...");
                if let Err(e) = table
                    .create_index(&["subject"], Index::FTS(FtsIndexBuilder::default()))
                    .execute()
                    .await
                {
                    debug!("emails.subject FTS index: {}", e);
                }
            }

            if !existing_columns.contains("body_plain") {
                info!("Creating FTS index on emails.body_plain...");
                if let Err(e) = table
                    .create_index(&["body_plain"], Index::FTS(FtsIndexBuilder::default()))
                    .execute()
                    .await
                {
                    debug!("emails.body_plain FTS index: {}", e);
                }
            }

            if !existing_columns.contains("id") {
                info!("Creating BTree index on emails.id...");
                if let Err(e) = table
                    .create_index(&["id"], Index::BTree(Default::default()))
                    .execute()
                    .await
                {
                    debug!("emails.id index: {}", e);
                }
            }
        }

        // Events table indexes
        if let Ok(table) = self.events_table() {
            let existing_indices = table.list_indices().await.unwrap_or_default();
            let existing_columns: std::collections::HashSet<_> = existing_indices
                .iter()
                .flat_map(|idx| idx.columns.clone())
                .collect();

            if !existing_columns.contains("id") {
                info!("Creating BTree index on events.id...");
                if let Err(e) = table
                    .create_index(&["id"], Index::BTree(Default::default()))
                    .execute()
                    .await
                {
                    debug!("events.id index: {}", e);
                }
            }
        }

        debug!("Index check complete");
        Ok(())
    }

    /// Rebuild FTS indexes to include newly added data
    /// FTS indexes in LanceDB are not automatically updated when data is added,
    /// so this should be called after sync batches complete.
    pub async fn rebuild_fts_indexes(&self) -> Result<()> {
        info!("Rebuilding FTS indexes...");
        let start = std::time::Instant::now();

        // Rebuild emails FTS indexes
        if let Ok(table) = self.emails_table() {
            // create_index replaces existing index
            if let Err(e) = table
                .create_index(&["subject"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await
            {
                debug!("Failed to rebuild emails.subject FTS index: {}", e);
            }
            if let Err(e) = table
                .create_index(&["body_plain"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await
            {
                debug!("Failed to rebuild emails.body_plain FTS index: {}", e);
            }
        }

        // Rebuild events FTS indexes
        if let Ok(table) = self.events_table() {
            if let Err(e) = table
                .create_index(&["summary"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await
            {
                debug!("Failed to rebuild events.summary FTS index: {}", e);
            }
            if let Err(e) = table
                .create_index(&["description"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await
            {
                debug!("Failed to rebuild events.description FTS index: {}", e);
            }
        }

        info!("FTS index rebuild complete in {:?}", start.elapsed());
        Ok(())
    }

    /// Get the emails table
    pub fn emails_table(&self) -> Result<Table> {
        self.emails
            .read()
            .clone()
            .ok_or_else(|| Error::TableNotFound(EMAILS_TABLE.to_string()))
    }

    /// Get the events table
    pub fn events_table(&self) -> Result<Table> {
        self.events
            .read()
            .clone()
            .ok_or_else(|| Error::TableNotFound(EVENTS_TABLE.to_string()))
    }

    /// Get the accounts table
    pub fn accounts_table(&self) -> Result<Table> {
        self.accounts
            .read()
            .clone()
            .ok_or_else(|| Error::TableNotFound(ACCOUNTS_TABLE.to_string()))
    }

    /// Insert or update an email
    pub async fn upsert_email(&self, email: &Email) -> Result<()> {
        let table = self.emails_table()?;
        let batch = email_to_batch(email)?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(email_schema()));

        // Delete existing if present
        table
            .delete(&format!("id = '{}'", email.id))
            .await
            .ok(); // Ignore if not found

        // Insert new
        table
            .add(Box::new(batches))
            .execute()
            .await?;

        debug!("Upserted email {}", email.id);
        Ok(())
    }

    /// Insert or update multiple emails
    pub async fn upsert_emails(&self, emails: &[Email]) -> Result<()> {
        if emails.is_empty() {
            return Ok(());
        }

        let table = self.emails_table()?;

        // Delete existing
        let ids: Vec<String> = emails.iter().map(|e| format!("'{}'", e.id)).collect();
        let filter = format!("id IN ({})", ids.join(", "));
        table.delete(&filter).await.ok();

        // Insert new
        let batch = emails_to_batch(emails)?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(email_schema()));
        table
            .add(Box::new(batches))
            .execute()
            .await?;

        debug!("Upserted {} emails", emails.len());
        Ok(())
    }

    /// Insert or update a calendar event
    pub async fn upsert_event(&self, event: &CalendarEvent) -> Result<()> {
        self.upsert_events(&[event.clone()]).await
    }

    /// Insert or update multiple calendar events
    pub async fn upsert_events(&self, events: &[CalendarEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        let table = self.events_table()?;

        // Delete existing
        let ids: Vec<String> = events.iter().map(|e| format!("'{}'", e.id)).collect();
        let filter = format!("id IN ({})", ids.join(", "));
        table.delete(&filter).await.ok();

        // Insert new
        let batch = events_to_batch(events)?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(event_schema()));
        table
            .add(Box::new(batches))
            .execute()
            .await?;

        debug!("Upserted {} events", events.len());
        Ok(())
    }

    /// Insert or update an account
    pub async fn upsert_account(&self, account: &Account) -> Result<()> {
        let table = self.accounts_table()?;

        // Read existing account data as backup before deletion
        let existing = self.get_account(&account.id).await?;

        // Prepare the new batch
        let batch = account_to_batch(account)?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(account_schema()));

        // Delete existing if present
        table
            .delete(&format!("id = '{}'", account.id))
            .await
            .ok();

        // Try to insert new data
        let result = table
            .add(Box::new(batches))
            .execute()
            .await;

        // If insert fails, try to restore the old data
        if let Err(e) = result {
            tracing::error!("Failed to insert account {}: {}", account.id, e);
            if let Some(old_account) = existing {
                tracing::warn!("Attempting to restore previous account data for {}", account.id);
                // Try to restore the old account data
                let restore_batch = account_to_batch(&old_account)?;
                let restore_batches = RecordBatchIterator::new(vec![Ok(restore_batch)], Arc::new(account_schema()));
                if let Err(restore_err) = table.add(Box::new(restore_batches)).execute().await {
                    tracing::error!("Failed to restore account {}: {} - DATA LOSS OCCURRED", account.id, restore_err);
                } else {
                    tracing::info!("Successfully restored previous account data for {}", account.id);
                }
            }
            return Err(Error::Database(e));
        }

        debug!("Upserted account {}", account.id);
        Ok(())
    }

    /// Get an email by ID
    pub async fn get_email(&self, id: &str) -> Result<Option<Email>> {
        let table = self.emails_table()?;
        let results = table
            .query()
            .only_if(&format!("id = '{}'", id))
            .limit(1)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Ok(None);
        }

        let email = batch_to_email(&batches[0], 0)?;
        Ok(Some(email))
    }

    /// Get multiple emails by ID in a single query (batch fetch)
    pub async fn get_emails_batch(&self, ids: &[String]) -> Result<Vec<Email>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let table = self.emails_table()?;

        // Build IN clause: id IN ('id1', 'id2', ...)
        let id_list: Vec<String> = ids.iter().map(|id| format!("'{}'", id)).collect();
        let filter = format!("id IN ({})", id_list.join(", "));

        let results = table
            .query()
            .only_if(&filter)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut emails = Vec::with_capacity(ids.len());
        for batch in &batches {
            for i in 0..batch.num_rows() {
                emails.push(batch_to_email(batch, i)?);
            }
        }

        Ok(emails)
    }

    /// Get all emails in a thread by gmail_thread_id
    pub async fn get_emails_by_thread(
        &self,
        thread_id: u64,
        account_id: Option<&str>,
    ) -> Result<Vec<Email>> {
        let table = self.emails_table()?;

        let mut filter = format!("gmail_thread_id = {}", thread_id);
        if let Some(acct) = account_id {
            filter.push_str(&format!(" AND account_id = '{}'", acct));
        }

        let results = table
            .query()
            .only_if(&filter)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut emails = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                emails.push(batch_to_email(batch, i)?);
            }
        }

        // Sort by date ascending (oldest first for thread view)
        emails.sort_by(|a, b| a.date.cmp(&b.date));

        Ok(emails)
    }

    /// Get emails that have attachments but haven't been downloaded yet
    pub async fn get_emails_with_pending_attachments(&self, account_id: &str) -> Result<Vec<Email>> {
        let table = self.emails_table()?;

        // Query emails with non-empty attachments
        // We filter for emails that have attachments JSON and check in Rust if downloaded
        let filter = format!(
            "account_id = '{}' AND attachments IS NOT NULL",
            account_id
        );

        let results = table
            .query()
            .only_if(&filter)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut pending_emails = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                let email = batch_to_email(batch, i)?;
                // Check if any attachment hasn't been downloaded
                let has_pending = email.attachments.iter().any(|att| !att.downloaded);
                if has_pending && !email.attachments.is_empty() {
                    pending_emails.push(email);
                }
            }
        }

        Ok(pending_emails)
    }

    /// Get attachment statistics for an account
    /// Returns (total_attachments, downloaded_attachments, total_size_bytes)
    pub async fn get_attachment_stats(&self, account_id: &str) -> Result<(usize, usize, u64)> {
        let table = self.emails_table()?;

        let filter = format!(
            "account_id = '{}' AND attachments IS NOT NULL",
            account_id
        );

        let results = table
            .query()
            .only_if(&filter)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut total = 0usize;
        let mut downloaded = 0usize;
        let mut total_size = 0u64;

        for batch in &batches {
            for i in 0..batch.num_rows() {
                let email = batch_to_email(batch, i)?;
                for att in &email.attachments {
                    total += 1;
                    if att.downloaded {
                        downloaded += 1;
                        total_size += att.size as u64;
                    }
                }
            }
        }

        Ok((total, downloaded, total_size))
    }

    /// Get an account by ID (email address)
    pub async fn get_account(&self, id: &str) -> Result<Option<Account>> {
        let table = self.accounts_table()?;
        let results = table
            .query()
            .only_if(&format!("id = '{}'", id))
            .limit(1)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Ok(None);
        }

        let account = batch_to_account(&batches[0], 0)?;
        Ok(Some(account))
    }

    /// Get an event by ID
    pub async fn get_event(&self, id: &str) -> Result<Option<CalendarEvent>> {
        let table = self.events_table()?;
        let results = table
            .query()
            .only_if(&format!("id = '{}'", id))
            .limit(1)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Ok(None);
        }

        let event = batch_to_event(&batches[0], 0)?;
        Ok(Some(event))
    }

    /// Get multiple events by ID in a single query (batch fetch)
    pub async fn get_events_batch(&self, ids: &[String]) -> Result<Vec<CalendarEvent>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let table = self.events_table()?;

        // Build IN clause: id IN ('id1', 'id2', ...)
        let id_list: Vec<String> = ids.iter().map(|id| format!("'{}'", id)).collect();
        let filter = format!("id IN ({})", id_list.join(", "));

        let results = table
            .query()
            .only_if(&filter)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut events = Vec::with_capacity(ids.len());
        for batch in &batches {
            for i in 0..batch.num_rows() {
                events.push(batch_to_event(batch, i)?);
            }
        }

        Ok(events)
    }

    /// List all accounts
    pub async fn list_accounts(&self) -> Result<Vec<Account>> {
        let table = self.accounts_table()?;
        let results = table
            .query()
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        let mut accounts = Vec::new();

        for batch in &batches {
            for i in 0..batch.num_rows() {
                accounts.push(batch_to_account(batch, i)?);
            }
        }

        Ok(accounts)
    }

    /// Clear all synced data for an account (keeps the account, clears emails/events)
    pub async fn clear_account_sync_data(&self, account_id: &str) -> Result<(u64, u64)> {
        let email_count = self.clear_account_emails(account_id).await?;
        let event_count = self.clear_account_events(account_id).await?;
        Ok((email_count, event_count))
    }

    /// Clear only emails for an account
    pub async fn clear_account_emails(&self, account_id: &str) -> Result<u64> {
        let email_count = self.count_emails(Some(account_id)).await?;
        let emails_table = self.emails_table()?;
        emails_table
            .delete(&format!("account_id = '{}'", account_id))
            .await?;
        info!("Cleared {} emails for account {}", email_count, account_id);
        Ok(email_count)
    }

    /// Clear only calendar events for an account
    pub async fn clear_account_events(&self, account_id: &str) -> Result<u64> {
        let event_count = self.count_events(Some(account_id)).await?;
        let events_table = self.events_table()?;
        events_table
            .delete(&format!("account_id = '{}'", account_id))
            .await?;
        info!("Cleared {} events for account {}", event_count, account_id);
        Ok(event_count)
    }

    /// Delete an account and all its data
    pub async fn delete_account(&self, account_id: &str) -> Result<()> {
        // Delete emails
        let emails_table = self.emails_table()?;
        emails_table
            .delete(&format!("account_id = '{}'", account_id))
            .await?;

        // Delete events
        let events_table = self.events_table()?;
        events_table
            .delete(&format!("account_id = '{}'", account_id))
            .await?;

        // Delete account
        let accounts_table = self.accounts_table()?;
        accounts_table
            .delete(&format!("id = '{}'", account_id))
            .await?;

        info!("Deleted account {} and all associated data", account_id);
        Ok(())
    }

    /// Count emails, optionally filtered by account
    pub async fn count_emails(&self, account_id: Option<&str>) -> Result<u64> {
        let table = self.emails_table()?;
        let query = match account_id {
            Some(id) => table.query().only_if(&format!("account_id = '{}'", id)),
            None => table.query(),
        };

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;
        let count: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
        Ok(count)
    }

    /// Count events, optionally filtered by account
    pub async fn count_events(&self, account_id: Option<&str>) -> Result<u64> {
        let table = self.events_table()?;
        let query = match account_id {
            Some(id) => table.query().only_if(&format!("account_id = '{}'", id)),
            None => table.query(),
        };

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;
        let count: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
        Ok(count)
    }

    /// Get the oldest email date for an account (for resume logic)
    pub async fn get_oldest_email_date(&self, account_id: &str) -> Result<Option<DateTime<Utc>>> {
        let table = self.emails_table()?;

        let query = table
            .query()
            .select(lancedb::query::Select::columns(&["date"]))
            .only_if(&format!("account_id = '{}'", account_id));

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut oldest: Option<DateTime<Utc>> = None;
        for batch in &batches {
            if let Some(date_col) = batch.column_by_name("date")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            {
                for i in 0..batch.num_rows() {
                    if let Some(ts) = DateTime::from_timestamp(date_col.value(i), 0) {
                        oldest = Some(match oldest {
                            Some(current) if ts < current => ts,
                            Some(current) => current,
                            None => ts,
                        });
                    }
                }
            }
        }

        Ok(oldest)
    }

    /// Get existing message_ids for an account (for deduplication during resume)
    pub async fn get_email_message_ids(&self, account_id: &str) -> Result<std::collections::HashSet<String>> {
        let table = self.emails_table()?;

        let query = table
            .query()
            .select(lancedb::query::Select::columns(&["message_id"]))
            .only_if(&format!("account_id = '{}'", account_id));

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut message_ids = std::collections::HashSet::new();
        for batch in &batches {
            if let Some(msg_id_col) = batch.column_by_name("message_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            {
                for i in 0..batch.num_rows() {
                    message_ids.insert(msg_id_col.value(i).to_string());
                }
            }
        }

        debug!("Loaded {} existing message_ids for {}", message_ids.len(), account_id);
        Ok(message_ids)
    }

    /// Get email sync boundaries for resume (oldest and newest dates)
    pub async fn get_email_sync_boundaries(&self, account_id: &str) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
        let table = self.emails_table()?;

        let query = table
            .query()
            .select(lancedb::query::Select::columns(&["date"]))
            .only_if(&format!("account_id = '{}'", account_id));

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut oldest: Option<DateTime<Utc>> = None;
        let mut newest: Option<DateTime<Utc>> = None;

        for batch in &batches {
            if let Some(date_col) = batch.column_by_name("date")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            {
                for i in 0..batch.num_rows() {
                    if let Some(ts) = DateTime::from_timestamp(date_col.value(i), 0) {
                        // Skip unreasonable dates to prevent bad data from breaking sync
                        if !is_reasonable_date(&ts) {
                            continue;
                        }
                        oldest = Some(match oldest {
                            Some(current) if ts < current => ts,
                            Some(current) => current,
                            None => ts,
                        });
                        newest = Some(match newest {
                            Some(current) if ts > current => ts,
                            Some(current) => current,
                            None => ts,
                        });
                    }
                }
            }
        }

        Ok((oldest, newest))
    }

    /// Get event sync boundaries for resume (oldest and newest dates)
    pub async fn get_event_sync_boundaries(&self, account_id: &str) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
        let table = self.events_table()?;

        let query = table
            .query()
            .select(lancedb::query::Select::columns(&["start"]))
            .only_if(&format!("account_id = '{}'", account_id));

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut oldest: Option<DateTime<Utc>> = None;
        let mut newest: Option<DateTime<Utc>> = None;

        for batch in &batches {
            if let Some(start_col) = batch.column_by_name("start")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            {
                for i in 0..batch.num_rows() {
                    let start_str = start_col.value(i);
                    // Parse as RFC3339 datetime or date-only
                    let dt = if let Ok(parsed) = DateTime::parse_from_rfc3339(start_str) {
                        parsed.with_timezone(&Utc)
                    } else if let Ok(date) = chrono::NaiveDate::parse_from_str(start_str, "%Y-%m-%d") {
                        date.and_hms_opt(0, 0, 0)
                            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                            .unwrap_or_else(Utc::now)
                    } else {
                        continue;
                    };

                    // Skip unreasonable dates to prevent bad data from breaking sync
                    if !is_reasonable_date(&dt) {
                        continue;
                    }

                    oldest = Some(match oldest {
                        Some(current) if dt < current => dt,
                        Some(current) => current,
                        None => dt,
                    });
                    newest = Some(match newest {
                        Some(current) if dt > current => dt,
                        Some(current) => current,
                        None => dt,
                    });
                }
            }
        }

        Ok((oldest, newest))
    }

    /// List recent emails sorted by date (newest first)
    /// This is optimized for listing without search - no embedding lookup
    pub async fn list_recent_emails(
        &self,
        account_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Email>> {
        let table = self.emails_table()?;

        // Select all columns except the embedding vector for speed
        let columns = &[
            "id", "account_id", "message_id", "gmail_thread_id", "folder",
            "subject", "from_email", "from_name", "to", "cc", "bcc",
            "date", "body_plain", "body_html", "snippet", "attachments",
            "labels", "flags", "uid",
        ];

        let mut query = table
            .query()
            .select(lancedb::query::Select::columns(columns));

        if let Some(id) = account_id {
            query = query.only_if(&format!("account_id = '{}'", id));
        }

        // LanceDB doesn't have ORDER BY in query API, so we fetch more and sort in memory
        // For better performance with large datasets, consider adding a date index
        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut emails = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                emails.push(batch_to_email(batch, i)?);
            }
        }

        // Sort by date descending (newest first)
        emails.sort_by(|a, b| b.date.cmp(&a.date));

        // Return only the requested limit
        emails.truncate(limit);

        Ok(emails)
    }

    /// List recent events sorted by start time (newest first)
    pub async fn list_recent_events(
        &self,
        account_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CalendarEvent>> {
        let table = self.events_table()?;

        // Select all columns except the embedding vector for speed
        let columns = &[
            "id", "account_id", "calendar_id", "ical_uid", "summary", "description",
            "location", "start_time", "end_time", "start_timestamp", "end_timestamp",
            "is_all_day", "recurrence_rule", "organizer", "attendees", "status",
            "created", "updated", "etag",
        ];

        let mut query = table
            .query()
            .select(lancedb::query::Select::columns(columns));

        if let Some(id) = account_id {
            query = query.only_if(&format!("account_id = '{}'", id));
        }

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut events = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                events.push(batch_to_event(batch, i)?);
            }
        }

        // Sort by start date descending (newest first)
        events.sort_by(|a, b| b.start.as_date().cmp(&a.start.as_date()));

        // Return only the requested limit
        events.truncate(limit);

        Ok(events)
    }

    /// List calendar events within a date range, sorted by start time (ascending).
    ///
    /// This is designed for answering questions like "what's on my calendar tomorrow"
    /// or "show me my meetings next week" without requiring a semantic search query.
    ///
    /// # Arguments
    /// * `accounts` - Optional list of account IDs to filter by
    /// * `from` - Start of date range (inclusive), as ISO 8601 date string (YYYY-MM-DD)
    /// * `to` - End of date range (exclusive), as ISO 8601 date string (YYYY-MM-DD)
    /// * `limit` - Maximum number of events to return
    ///
    /// # Returns
    /// Events sorted by start time ascending (chronological order)
    pub async fn list_events_in_range(
        &self,
        accounts: Option<&[String]>,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<CalendarEvent>> {
        let table = self.events_table()?;

        // Select all columns except the embedding vector for speed
        let columns = &[
            "id", "account_id", "calendar_id", "ical_uid", "summary", "description",
            "location", "start", "end", "timezone", "all_day",
            "recurrence_rule", "organizer", "attendees", "status",
            "etag",
        ];

        // Build filter for date range
        // start is stored as ISO 8601 string, so string comparison works
        let mut filters = vec![
            format!("start >= '{}'", from),
            format!("start < '{}'", to),
        ];

        // Add account filter if specified
        if let Some(accts) = accounts {
            if !accts.is_empty() {
                let account_list = accts
                    .iter()
                    .map(|a| format!("'{}'", a))
                    .collect::<Vec<_>>()
                    .join(", ");
                filters.push(format!("account_id IN ({})", account_list));
            }
        }

        let filter = filters.join(" AND ");

        let query = table
            .query()
            .select(lancedb::query::Select::columns(columns))
            .only_if(&filter);

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut events = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                events.push(batch_to_event(batch, i)?);
            }
        }

        // Sort by start time ascending (chronological order)
        events.sort_by(|a, b| a.start.as_date().cmp(&b.start.as_date()));

        // Return only the requested limit
        events.truncate(limit);

        Ok(events)
    }

    /// Get a map of google_event_id -> etag for all events in an account
    /// Used to detect which events have changed during incremental sync
    pub async fn get_event_etags(&self, account_id: &str) -> Result<std::collections::HashMap<String, String>> {
        let table = self.events_table()?;

        // Only fetch the fields we need for comparison
        let query = table
            .query()
            .select(lancedb::query::Select::columns(&["google_event_id", "etag"]))
            .only_if(&format!("account_id = '{}'", account_id));

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut etags = std::collections::HashMap::new();
        for batch in &batches {
            let google_event_ids = batch
                .column_by_name("google_event_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let etag_col = batch
                .column_by_name("etag")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());

            if let (Some(ids), Some(etag_arr)) = (google_event_ids, etag_col) {
                for i in 0..batch.num_rows() {
                    if let (Some(id), Some(etag)) = (ids.value(i).to_string().into(), etag_arr.value(i).to_string().into()) {
                        let id: String = ids.value(i).to_string();
                        let etag: String = etag_arr.value(i).to_string();
                        etags.insert(id, etag);
                    }
                }
            }
        }

        debug!("Loaded {} event etags for {}", etags.len(), account_id);
        Ok(etags)
    }
}

// Helper trait for collecting async streams
use futures::stream::TryStreamExt;
