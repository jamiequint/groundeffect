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
        let table = self.events_table()?;
        let batch = event_to_batch(event)?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], Arc::new(event_schema()));

        // Delete existing if present
        table
            .delete(&format!("id = '{}'", event.id))
            .await
            .ok();

        // Insert new
        table
            .add(Box::new(batches))
            .execute()
            .await?;

        debug!("Upserted event {}", event.id);
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
}

// Helper trait for collecting async streams
use futures::stream::TryStreamExt;
