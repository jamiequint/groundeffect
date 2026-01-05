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
            *self.accounts.write() = Some(table);
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
        let batch = account_to_batch(account)?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(account_schema()));

        // Delete existing if present
        table
            .delete(&format!("id = '{}'", account.id))
            .await
            .ok();

        // Insert new
        table
            .add(Box::new(batches))
            .execute()
            .await?;

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
        // Count before deleting
        let email_count = self.count_emails(Some(account_id)).await?;
        let event_count = self.count_events(Some(account_id)).await?;

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

        info!("Cleared {} emails and {} events for account {}", email_count, event_count, account_id);
        Ok((email_count, event_count))
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
            "id", "account_id", "message_id", "thread_id", "folder",
            "subject", "from_email", "from_name", "to_list", "cc_list", "bcc_list",
            "date", "body_plain", "body_html", "snippet", "has_attachments",
            "attachment_count", "labels", "is_read", "is_starred", "raw_headers",
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
