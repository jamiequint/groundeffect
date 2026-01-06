//! Hybrid search engine using LanceDB's built-in BM25 and vector search
//!
//! Combines BM25 full-text search with vector similarity search using
//! Reciprocal Rank Fusion (RRF) for optimal results.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Table;
use tracing::{debug, info};

use crate::db::Database;
use crate::embedding::EmbeddingEngine;
use crate::error::Result;
use crate::models::{CalendarEvent, EmailSearchResult, EmailSummary};

/// RRF constant (standard value is 60)
const RRF_K: f32 = 60.0;

/// Search options
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Account IDs to search (None = all accounts)
    pub accounts: Option<Vec<String>>,

    /// Maximum number of results
    pub limit: usize,

    /// Filter by folder
    pub folder: Option<String>,

    /// Filter by sender (email or name)
    pub from: Option<String>,

    /// Filter by recipient
    pub to: Option<String>,

    /// Filter by date (after)
    pub date_from: Option<chrono::DateTime<chrono::Utc>>,

    /// Filter by date (before)
    pub date_to: Option<chrono::DateTime<chrono::Utc>>,

    /// Filter by attachment presence
    pub has_attachment: Option<bool>,

    /// BM25 weight (0.0-1.0)
    pub bm25_weight: f32,

    /// Vector weight (0.0-1.0)
    pub vector_weight: f32,
}

impl SearchOptions {
    /// Create with defaults
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            bm25_weight: 0.5,
            vector_weight: 0.5,
            ..Default::default()
        }
    }

    /// Build a SQL WHERE clause from the filters
    pub fn build_filter(&self) -> Option<String> {
        let mut conditions = Vec::new();

        // Account filter
        if let Some(accounts) = &self.accounts {
            if !accounts.is_empty() {
                let account_list: Vec<String> = accounts.iter().map(|a| format!("'{}'", a)).collect();
                conditions.push(format!("account_id IN ({})", account_list.join(", ")));
            }
        }

        // Folder filter
        if let Some(folder) = &self.folder {
            conditions.push(format!("folder = '{}'", folder));
        }

        // From filter (contains match)
        if let Some(from) = &self.from {
            conditions.push(format!(
                "(from_email LIKE '%{}%' OR from_name LIKE '%{}%')",
                from, from
            ));
        }

        // To filter (contains match in JSON array)
        if let Some(to) = &self.to {
            conditions.push(format!("to LIKE '%{}%'", to));
        }

        // Date filters
        if let Some(date_from) = &self.date_from {
            conditions.push(format!("date >= {}", date_from.timestamp()));
        }
        if let Some(date_to) = &self.date_to {
            conditions.push(format!("date <= {}", date_to.timestamp()));
        }

        // Attachment filter
        if let Some(has_attachment) = &self.has_attachment {
            if *has_attachment {
                // Has attachments: non-empty JSON array (not null and not "[]")
                conditions.push("attachments IS NOT NULL AND attachments != '[]'".to_string());
            } else {
                // No attachments: null or empty JSON array
                conditions.push("(attachments IS NULL OR attachments = '[]')".to_string());
            }
        }

        if conditions.is_empty() {
            None
        } else {
            Some(conditions.join(" AND "))
        }
    }
}

/// Hybrid search engine
pub struct SearchEngine {
    db: Arc<Database>,
    embedding_engine: Arc<EmbeddingEngine>,
}

impl SearchEngine {
    /// Create a new search engine
    pub fn new(db: Arc<Database>, embedding_engine: Arc<EmbeddingEngine>) -> Self {
        Self {
            db,
            embedding_engine,
        }
    }

    /// Search emails using hybrid BM25 + vector search
    pub async fn search_emails(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<EmailSearchResult>> {
        info!("Searching emails: query='{}', limit={}", query, options.limit);

        let table = self.db.emails_table()?;
        let filter = options.build_filter();

        debug!("Search filter: {:?}", filter);

        // Run BM25 and vector search in parallel
        let search_start = std::time::Instant::now();
        let (bm25_results, vector_results) = tokio::join!(
            self.bm25_search_emails(&table, query, &filter, options.limit * 2),
            self.vector_search_emails(&table, query, &filter, options.limit * 2)
        );
        info!("Search phase took {:?}", search_start.elapsed());
        let bm25_results = bm25_results?;
        let vector_results = vector_results?;

        // Combine using RRF
        let combined = self.rrf_fusion(
            &bm25_results,
            &vector_results,
            options.bm25_weight,
            options.vector_weight,
        );

        // Get top result IDs and scores
        let top_results: Vec<(String, f32)> = combined.into_iter().take(options.limit).collect();
        let ids: Vec<String> = top_results.iter().map(|(id, _)| id.clone()).collect();

        // Batch fetch all emails in a single query
        let fetch_start = std::time::Instant::now();
        let emails = self.db.get_emails_batch(&ids).await?;
        info!("Batch fetch of {} emails took {:?}", ids.len(), fetch_start.elapsed());

        // Build a map for O(1) lookup while preserving RRF order
        let email_map: std::collections::HashMap<String, _> = emails
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();

        // Build results in RRF-ranked order
        let mut results = Vec::with_capacity(top_results.len());
        for (id, score) in top_results {
            if let Some(email) = email_map.get(&id) {
                let summary = EmailSummary::from(email);
                results.push(EmailSearchResult {
                    email: summary,
                    score,
                    markdown_summary: email.markdown_summary(),
                });
            }
        }

        debug!("Found {} email results", results.len());
        Ok(results)
    }

    /// BM25 full-text search
    async fn bm25_search_emails(
        &self,
        table: &Table,
        query: &str,
        filter: &Option<String>,
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        use futures::TryStreamExt;

        let start = std::time::Instant::now();
        let fts_query = FullTextSearchQuery::new(query.to_owned());
        let mut search = table.query().full_text_search(fts_query);

        if let Some(f) = filter {
            search = search.only_if(f);
        }

        let results = search
            .limit(limit)
            .select(lancedb::query::Select::columns(&["id", "_score"]))
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut scored_results = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                if let Some(id_col) = batch.column_by_name("id") {
                    if let Some(id_array) = id_col.as_any().downcast_ref::<arrow_array::StringArray>() {
                        let id = id_array.value(row).to_string();
                        // Use _score from BM25 if available, otherwise use rank
                        let score = if let Some(score_col) = batch.column_by_name("_score") {
                            if let Some(score_array) = score_col.as_any().downcast_ref::<arrow_array::Float32Array>() {
                                score_array.value(row)
                            } else {
                                1.0 / (row as f32 + 1.0)
                            }
                        } else {
                            1.0 / (row as f32 + 1.0)
                        };
                        scored_results.push((id, score));
                    }
                }
            }
        }

        info!("BM25 search took {:?}, found {} results", start.elapsed(), scored_results.len());
        Ok(scored_results)
    }

    /// Vector similarity search
    async fn vector_search_emails(
        &self,
        table: &Table,
        query: &str,
        filter: &Option<String>,
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        use futures::TryStreamExt;

        let start = std::time::Instant::now();

        // Generate query embedding
        let embed_start = std::time::Instant::now();
        let query_embedding = self.embedding_engine.embed(query)?;
        info!("Query embedding took {:?}", embed_start.elapsed());

        let mut search = table.vector_search(query_embedding)?;

        if let Some(f) = filter {
            search = search.only_if(f);
        }

        let results = search
            .limit(limit)
            .select(lancedb::query::Select::columns(&["id", "_distance"]))
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut scored_results = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                if let Some(id_col) = batch.column_by_name("id") {
                    if let Some(id_array) = id_col.as_any().downcast_ref::<arrow_array::StringArray>() {
                        let id = id_array.value(row).to_string();

                        // Get distance score - convert to similarity (lower distance = higher similarity)
                        let score = if let Some(dist_col) = batch.column_by_name("_distance") {
                            if let Some(dist_array) = dist_col.as_any().downcast_ref::<arrow_array::Float32Array>() {
                                1.0 / (1.0 + dist_array.value(row))
                            } else {
                                1.0 / (row as f32 + 1.0)
                            }
                        } else {
                            1.0 / (row as f32 + 1.0)
                        };

                        scored_results.push((id, score));
                    }
                }
            }
        }

        info!("Vector search took {:?}, found {} results", start.elapsed(), scored_results.len());
        Ok(scored_results)
    }

    /// Combine results using Reciprocal Rank Fusion
    fn rrf_fusion(
        &self,
        bm25_results: &[(String, f32)],
        vector_results: &[(String, f32)],
        bm25_weight: f32,
        vector_weight: f32,
    ) -> Vec<(String, f32)> {
        let mut scores: HashMap<String, f32> = HashMap::new();

        // Add BM25 scores
        for (rank, (id, _score)) in bm25_results.iter().enumerate() {
            let rrf_score = bm25_weight / (RRF_K + rank as f32 + 1.0);
            *scores.entry(id.clone()).or_default() += rrf_score;
        }

        // Add vector scores
        for (rank, (id, _score)) in vector_results.iter().enumerate() {
            let rrf_score = vector_weight / (RRF_K + rank as f32 + 1.0);
            *scores.entry(id.clone()).or_default() += rrf_score;
        }

        // Sort by combined score
        let mut results: Vec<(String, f32)> = scores.into_iter().collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        results
    }

    /// Search calendar events using hybrid BM25 + vector search
    pub async fn search_calendar(
        &self,
        query: &str,
        options: &CalendarSearchOptions,
    ) -> Result<Vec<CalendarSearchResult>> {
        info!("Searching calendar: query='{}', limit={}", query, options.limit);

        let table = self.db.events_table()?;
        let filter = options.build_filter();

        // Run BM25 and vector search in parallel
        let (bm25_results, vector_results) = tokio::join!(
            self.bm25_search_events(&table, query, &filter, options.limit * 2),
            self.vector_search_events(&table, query, &filter, options.limit * 2)
        );
        let bm25_results = bm25_results?;
        let vector_results = vector_results?;

        // Combine using RRF
        let combined = self.rrf_fusion(
            &bm25_results,
            &vector_results,
            0.5,
            0.5,
        );

        // Get top result IDs and scores
        let top_results: Vec<(String, f32)> = combined.into_iter().take(options.limit).collect();
        let ids: Vec<String> = top_results.iter().map(|(id, _)| id.clone()).collect();

        // Batch fetch all events in a single query
        let events = self.db.get_events_batch(&ids).await?;

        // Build a map for O(1) lookup while preserving RRF order
        let event_map: std::collections::HashMap<String, _> = events
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();

        // Build results in RRF-ranked order
        let mut results = Vec::with_capacity(top_results.len());
        for (id, score) in top_results {
            if let Some(event) = event_map.get(&id) {
                results.push(CalendarSearchResult { event: event.clone(), score });
            }
        }

        debug!("Found {} calendar results", results.len());
        Ok(results)
    }

    /// BM25 full-text search for events
    async fn bm25_search_events(
        &self,
        table: &Table,
        query: &str,
        filter: &Option<String>,
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        use futures::TryStreamExt;

        let fts_query = FullTextSearchQuery::new(query.to_owned());
        let mut search = table.query().full_text_search(fts_query);

        if let Some(f) = filter {
            search = search.only_if(f);
        }

        let results = search
            .limit(limit)
            .select(lancedb::query::Select::columns(&["id", "_score"]))
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut scored_results = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                if let Some(id_col) = batch.column_by_name("id") {
                    if let Some(id_array) = id_col.as_any().downcast_ref::<arrow_array::StringArray>() {
                        let id = id_array.value(row).to_string();
                        let score = if let Some(score_col) = batch.column_by_name("_score") {
                            if let Some(score_array) = score_col.as_any().downcast_ref::<arrow_array::Float32Array>() {
                                score_array.value(row)
                            } else {
                                1.0 / (row as f32 + 1.0)
                            }
                        } else {
                            1.0 / (row as f32 + 1.0)
                        };
                        scored_results.push((id, score));
                    }
                }
            }
        }

        Ok(scored_results)
    }

    /// Vector similarity search for events
    async fn vector_search_events(
        &self,
        table: &Table,
        query: &str,
        filter: &Option<String>,
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        use futures::TryStreamExt;

        let query_embedding = self.embedding_engine.embed(query)?;

        let mut search = table.vector_search(query_embedding)?;

        if let Some(f) = filter {
            search = search.only_if(f);
        }

        let results = search
            .limit(limit)
            .select(lancedb::query::Select::columns(&["id", "_distance"]))
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut scored_results = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                if let Some(id_col) = batch.column_by_name("id") {
                    if let Some(id_array) = id_col.as_any().downcast_ref::<arrow_array::StringArray>() {
                        let id = id_array.value(row).to_string();

                        let score = if let Some(dist_col) = batch.column_by_name("_distance") {
                            if let Some(dist_array) = dist_col.as_any().downcast_ref::<arrow_array::Float32Array>() {
                                1.0 / (1.0 + dist_array.value(row))
                            } else {
                                1.0 / (row as f32 + 1.0)
                            }
                        } else {
                            1.0 / (row as f32 + 1.0)
                        };

                        scored_results.push((id, score));
                    }
                }
            }
        }

        Ok(scored_results)
    }
}

/// Calendar search options
#[derive(Debug, Clone, Default)]
pub struct CalendarSearchOptions {
    /// Account IDs to search (None = all accounts)
    pub accounts: Option<Vec<String>>,

    /// Maximum number of results
    pub limit: usize,

    /// Filter by calendar ID
    pub calendar_id: Option<String>,

    /// Filter by date (after)
    pub date_from: Option<chrono::DateTime<chrono::Utc>>,

    /// Filter by date (before)
    pub date_to: Option<chrono::DateTime<chrono::Utc>>,
}

impl CalendarSearchOptions {
    /// Build a SQL WHERE clause from the filters
    pub fn build_filter(&self) -> Option<String> {
        let mut conditions = Vec::new();

        if let Some(accounts) = &self.accounts {
            if !accounts.is_empty() {
                let account_list: Vec<String> = accounts.iter().map(|a| format!("'{}'", a)).collect();
                conditions.push(format!("account_id IN ({})", account_list.join(", ")));
            }
        }

        if let Some(calendar_id) = &self.calendar_id {
            conditions.push(format!("calendar_id = '{}'", calendar_id));
        }

        // Date filters compare ISO 8601 strings (which sort correctly)
        if let Some(date_from) = &self.date_from {
            conditions.push(format!("start >= '{}'", date_from.format("%Y-%m-%d")));
        }
        if let Some(date_to) = &self.date_to {
            // Use next day to include all events on date_to
            let next_day = *date_to + chrono::Duration::days(1);
            conditions.push(format!("start < '{}'", next_day.format("%Y-%m-%d")));
        }

        if conditions.is_empty() {
            None
        } else {
            Some(conditions.join(" AND "))
        }
    }
}

/// Calendar search result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CalendarSearchResult {
    /// The event
    #[serde(flatten)]
    pub event: CalendarEvent,

    /// Search relevance score
    pub score: f32,
}

/// Search response for MCP
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResponse<T> {
    /// Search results
    pub results: Vec<T>,

    /// Accounts that were searched
    pub accounts_searched: Vec<String>,

    /// Total number of matching results
    pub total_count: usize,

    /// Search time in milliseconds
    pub search_time_ms: u64,
}
