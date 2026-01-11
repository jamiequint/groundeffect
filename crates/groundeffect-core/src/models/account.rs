//! Account data structures

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Status of an account
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// Account is active and syncing normally
    Active,
    /// Account requires re-authentication
    NeedsReauth,
    /// Account is disabled by user
    Disabled,
    /// Account is currently syncing
    Syncing,
}

impl Default for AccountStatus {
    fn default() -> Self {
        Self::Active
    }
}

/// A connected Gmail/Google Calendar account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Email address (primary key)
    pub id: String,

    /// User-defined alias (e.g., "work", "personal")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Display name from Google profile
    pub display_name: String,

    /// When the account was added
    pub added_at: DateTime<Utc>,

    /// Last successful email sync time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_email: Option<DateTime<Utc>>,

    /// Last successful calendar sync time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_calendar: Option<DateTime<Utc>>,

    /// Current account status
    pub status: AccountStatus,

    /// Earliest date to sync emails from (for initial sync)
    /// None means use default (90 days back)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_email_since: Option<DateTime<Utc>>,

    /// Oldest date we've synced emails back to
    /// Used to track what's been synced for historical sync
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_email_synced: Option<DateTime<Utc>>,

    /// Oldest date we've synced calendar events back to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_event_synced: Option<DateTime<Utc>>,

    /// Whether to sync email attachments for this account
    #[serde(default)]
    pub sync_attachments: bool,

    /// Estimated total emails on IMAP server for the sync period
    /// Updated by daemon when counting emails to sync
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_total_emails: Option<u64>,
}

impl Account {
    /// Create a new account
    pub fn new(email: String, display_name: String) -> Self {
        Self {
            id: email,
            alias: None,
            display_name,
            added_at: Utc::now(),
            last_sync_email: None,
            last_sync_calendar: None,
            status: AccountStatus::Active,
            sync_email_since: None,
            oldest_email_synced: None,
            oldest_event_synced: None,
            sync_attachments: false,
            estimated_total_emails: None,
        }
    }

    /// Enable attachment syncing
    pub fn with_sync_attachments(mut self, enabled: bool) -> Self {
        self.sync_attachments = enabled;
        self
    }

    /// Set how many years back to sync emails
    pub fn with_years_to_sync(mut self, years: u32) -> Self {
        use chrono::Duration;
        let days = years as i64 * 365;
        self.sync_email_since = Some(Utc::now() - Duration::days(days));
        self
    }

    /// Set the account alias
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    /// Check if this account matches an identifier (email or alias)
    pub fn matches(&self, identifier: &str) -> bool {
        self.id.eq_ignore_ascii_case(identifier)
            || self
                .alias
                .as_ref()
                .map(|a| a.eq_ignore_ascii_case(identifier))
                .unwrap_or(false)
    }

    /// Get a display identifier (alias if set, otherwise email)
    pub fn display_id(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.id)
    }
}

/// Summary statistics for an account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountStats {
    /// Account ID (email address)
    pub id: String,

    /// Account alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Current status
    pub status: AccountStatus,

    /// Last email sync time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_email_sync: Option<DateTime<Utc>>,

    /// Last calendar sync time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_calendar_sync: Option<DateTime<Utc>>,

    /// Number of emails indexed
    pub email_count: u64,

    /// Number of calendar events indexed
    pub event_count: u64,

    /// Number of attachments indexed
    pub attachment_count: u64,
}

/// Aggregate statistics across all accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotalStats {
    /// Total emails across all accounts
    pub email_count: u64,

    /// Total events across all accounts
    pub event_count: u64,

    /// Total attachments across all accounts
    pub attachment_count: u64,

    /// Total index size in MB
    pub index_size_mb: f64,

    /// Total attachment storage in MB
    pub attachment_storage_mb: f64,
}

/// Sync status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    /// Per-account status
    pub accounts: Vec<AccountStats>,

    /// Aggregate totals
    pub totals: TotalStats,
}
