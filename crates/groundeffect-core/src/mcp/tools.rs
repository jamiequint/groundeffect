//! MCP tool implementations

use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, info};

use crate::config::Config;
use crate::db::Database;
use crate::error::{Error, Result};
use crate::models::SendEmailRequest;
use crate::oauth::OAuthManager;
use crate::search::{CalendarSearchOptions, SearchEngine, SearchOptions};
use super::protocol::{ToolDefinition, ToolResult};

/// Get all tool definitions
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        // Account tools
        ToolDefinition {
            name: "list_accounts".to_string(),
            description: "List all connected Gmail/GCal accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "get_account".to_string(),
            description: "Get details for a specific account".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account email or alias"
                    }
                },
                "required": ["account"]
            }),
        },
        // Email tools
        ToolDefinition {
            name: "search_emails".to_string(),
            description: "Search emails using hybrid BM25 + vector search across one or more accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (natural language)"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Account(s) to search (email addresses or aliases). Omit to search ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100
                    },
                    "folder": {
                        "type": "string",
                        "description": "Filter by folder (e.g., INBOX, Sent)"
                    },
                    "from": {
                        "type": "string",
                        "description": "Filter by sender email/name"
                    },
                    "to": {
                        "type": "string",
                        "description": "Filter by recipient email/name"
                    },
                    "date_from": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter emails after this date"
                    },
                    "date_to": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter emails before this date"
                    },
                    "has_attachment": {
                        "type": "boolean",
                        "description": "Filter emails with attachments"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_email".to_string(),
            description: "Fetch single email by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Email ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "get_thread".to_string(),
            description: "Fetch all emails in a thread".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "thread_id": {
                        "type": "string",
                        "description": "Gmail thread ID"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                },
                "required": ["thread_id"]
            }),
        },
        ToolDefinition {
            name: "list_folders".to_string(),
            description: "List all IMAP folders".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        // Calendar tools
        ToolDefinition {
            name: "search_calendar".to_string(),
            description: "Search calendar events across one or more accounts".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (natural language)"
                    },
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Account(s) to search. Omit to search ALL accounts."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "maximum": 100
                    },
                    "calendar_id": {
                        "type": "string",
                        "description": "Filter to specific calendar"
                    },
                    "date_from": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter events after this date"
                    },
                    "date_to": {
                        "type": "string",
                        "format": "date",
                        "description": "Filter events before this date"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_event".to_string(),
            description: "Fetch single calendar event by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Event ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "list_calendars".to_string(),
            description: "List all calendars".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "create_event".to_string(),
            description: "Create a new calendar event".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account to create event on (email or alias)"
                    },
                    "summary": {
                        "type": "string",
                        "description": "Event title"
                    },
                    "start": {
                        "type": "string",
                        "format": "date-time",
                        "description": "Start time (ISO 8601)"
                    },
                    "end": {
                        "type": "string",
                        "format": "date-time",
                        "description": "End time (ISO 8601)"
                    },
                    "calendar_id": {
                        "type": "string",
                        "description": "Calendar ID (omit for primary)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Event description"
                    },
                    "location": {
                        "type": "string",
                        "description": "Event location"
                    },
                    "attendees": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Attendee email addresses"
                    }
                },
                "required": ["account", "summary", "start", "end"]
            }),
        },
        // System tools
        ToolDefinition {
            name: "get_sync_status".to_string(),
            description: "Get current sync status and statistics".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accounts": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter to specific accounts"
                    }
                }
            }),
        },
    ]
}

/// Tool execution handler
pub struct ToolHandler {
    db: Arc<Database>,
    config: Arc<Config>,
    search: Arc<SearchEngine>,
    oauth: Arc<OAuthManager>,
}

impl ToolHandler {
    /// Create a new tool handler
    pub fn new(
        db: Arc<Database>,
        config: Arc<Config>,
        search: Arc<SearchEngine>,
        oauth: Arc<OAuthManager>,
    ) -> Self {
        Self {
            db,
            config,
            search,
            oauth,
        }
    }

    /// Execute a tool
    pub async fn execute(&self, name: &str, arguments: &Value) -> Result<Value> {
        debug!("Executing tool: {} with args: {:?}", name, arguments);

        let result = match name {
            "list_accounts" => self.list_accounts().await,
            "get_account" => self.get_account(arguments).await,
            "search_emails" => self.search_emails(arguments).await,
            "get_email" => self.get_email(arguments).await,
            "get_thread" => self.get_thread(arguments).await,
            "list_folders" => self.list_folders(arguments).await,
            "search_calendar" => self.search_calendar(arguments).await,
            "get_event" => self.get_event(arguments).await,
            "list_calendars" => self.list_calendars(arguments).await,
            "create_event" => self.create_event(arguments).await,
            "get_sync_status" => self.get_sync_status(arguments).await,
            _ => Err(Error::ToolNotFound(name.to_string())),
        }?;

        Ok(serde_json::json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&result)?
            }]
        }))
    }

    /// List all accounts
    async fn list_accounts(&self) -> Result<Value> {
        let accounts = self.db.list_accounts().await?;
        Ok(serde_json::json!({
            "accounts": accounts.iter().map(|a| serde_json::json!({
                "id": a.id,
                "alias": a.alias,
                "display_name": a.display_name,
                "status": format!("{:?}", a.status).to_lowercase(),
                "added_at": a.added_at.to_rfc3339()
            })).collect::<Vec<_>>()
        }))
    }

    /// Get a specific account
    async fn get_account(&self, args: &Value) -> Result<Value> {
        let account_id = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        // Resolve alias if needed
        let email = self
            .config
            .resolve_account(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        let account = self
            .db
            .get_account(&email)
            .await?
            .ok_or_else(|| Error::AccountNotFound(email.clone()))?;

        Ok(serde_json::to_value(&account)?)
    }

    /// Search emails
    async fn search_emails(&self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing query".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        // Resolve account aliases
        let accounts = args["accounts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|id| self.config.resolve_account(id))
                    .collect::<Vec<_>>()
            });

        let options = SearchOptions {
            accounts,
            limit,
            folder: args["folder"].as_str().map(|s| s.to_string()),
            from: args["from"].as_str().map(|s| s.to_string()),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let results = self.search.search_emails(query, &options).await?;
        let search_time = start.elapsed().as_millis();

        Ok(serde_json::json!({
            "results": results,
            "accounts_searched": options.accounts.unwrap_or_default(),
            "total_count": results.len(),
            "search_time_ms": search_time
        }))
    }

    /// Get a single email
    async fn get_email(&self, args: &Value) -> Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing id".to_string()))?;

        let email = self
            .db
            .get_email(id)
            .await?
            .ok_or_else(|| Error::EmailNotFound(id.to_string()))?;

        Ok(serde_json::to_value(&email)?)
    }

    /// Get all emails in a thread
    async fn get_thread(&self, args: &Value) -> Result<Value> {
        let thread_id = args["thread_id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing thread_id".to_string()))?;

        // TODO: Implement thread fetching from LanceDB
        // For now, return an error indicating the feature is pending
        Err(Error::Other("Thread fetching not yet implemented".to_string()))
    }

    /// List folders
    async fn list_folders(&self, _args: &Value) -> Result<Value> {
        // Return common Gmail folders
        Ok(serde_json::json!({
            "folders": [
                "INBOX",
                "[Gmail]/All Mail",
                "[Gmail]/Drafts",
                "[Gmail]/Important",
                "[Gmail]/Sent Mail",
                "[Gmail]/Spam",
                "[Gmail]/Starred",
                "[Gmail]/Trash"
            ]
        }))
    }

    /// Search calendar events
    async fn search_calendar(&self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing query".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        // Resolve account aliases
        let accounts = args["accounts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|id| self.config.resolve_account(id))
                    .collect::<Vec<_>>()
            });

        let options = CalendarSearchOptions {
            accounts,
            limit,
            calendar_id: args["calendar_id"].as_str().map(|s| s.to_string()),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let results = self.search.search_calendar(query, &options).await?;
        let search_time = start.elapsed().as_millis();

        Ok(serde_json::json!({
            "results": results,
            "accounts_searched": options.accounts.unwrap_or_default(),
            "total_count": results.len(),
            "search_time_ms": search_time
        }))
    }

    /// Get a single event
    async fn get_event(&self, args: &Value) -> Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing id".to_string()))?;

        let event = self
            .db
            .get_event(id)
            .await?
            .ok_or_else(|| Error::Other(format!("Event not found: {}", id)))?;

        Ok(serde_json::to_value(&event)?)
    }

    /// List calendars
    async fn list_calendars(&self, _args: &Value) -> Result<Value> {
        // TODO: Implement calendar listing
        Ok(serde_json::json!({
            "calendars": []
        }))
    }

    /// Create a calendar event
    async fn create_event(&self, args: &Value) -> Result<Value> {
        let account = args["account"]
            .as_str()
            .ok_or_else(|| Error::InvalidRequest("Missing account".to_string()))?;

        let account_email = self
            .config
            .resolve_account(account)
            .ok_or_else(|| Error::AccountNotFound(account.to_string()))?;

        // TODO: Implement event creation via CalDAV
        Err(Error::Other("Event creation not yet implemented".to_string()))
    }

    /// Get sync status
    async fn get_sync_status(&self, args: &Value) -> Result<Value> {
        let accounts = self.db.list_accounts().await?;

        let mut account_stats = Vec::new();
        let mut total_emails = 0u64;
        let mut total_events = 0u64;

        for account in &accounts {
            let email_count = self.db.count_emails(Some(&account.id)).await?;
            let event_count = self.db.count_events(Some(&account.id)).await?;

            total_emails += email_count;
            total_events += event_count;

            account_stats.push(serde_json::json!({
                "id": account.id,
                "alias": account.alias,
                "status": format!("{:?}", account.status).to_lowercase(),
                "last_email_sync": account.last_sync_email.map(|d| d.to_rfc3339()),
                "last_calendar_sync": account.last_sync_calendar.map(|d| d.to_rfc3339()),
                "email_count": email_count,
                "event_count": event_count,
                "attachment_count": 0 // TODO
            }));
        }

        Ok(serde_json::json!({
            "accounts": account_stats,
            "totals": {
                "email_count": total_emails,
                "event_count": total_events,
                "attachment_count": 0,
                "index_size_mb": 0.0,
                "attachment_storage_mb": 0.0
            }
        }))
    }
}
