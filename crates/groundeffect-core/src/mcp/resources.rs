//! MCP resource implementations

use std::sync::Arc;

use serde_json::Value;
use tracing::debug;

use crate::config::Config;
use crate::db::Database;
use crate::error::{Error, Result};

use super::protocol::ResourceDefinition;

/// Get all resource definitions
pub fn get_resource_definitions() -> Vec<ResourceDefinition> {
    vec![
        ResourceDefinition {
            uri: "email://{message_id}".to_string(),
            name: "Email content".to_string(),
            description: "Raw email content by message ID".to_string(),
            mime_type: Some("message/rfc822".to_string()),
        },
        ResourceDefinition {
            uri: "email://{message_id}/body".to_string(),
            name: "Email body".to_string(),
            description: "Email body as plain text".to_string(),
            mime_type: Some("text/plain".to_string()),
        },
        ResourceDefinition {
            uri: "email://{message_id}/attachments/{filename}".to_string(),
            name: "Email attachment".to_string(),
            description: "Download email attachment".to_string(),
            mime_type: None,
        },
        ResourceDefinition {
            uri: "calendar://{event_id}".to_string(),
            name: "Calendar event".to_string(),
            description: "Calendar event in iCalendar format".to_string(),
            mime_type: Some("text/calendar".to_string()),
        },
    ]
}

/// Resource handler
pub struct ResourceHandler {
    db: Arc<Database>,
    config: Arc<Config>,
}

impl ResourceHandler {
    /// Create a new resource handler
    pub fn new(db: Arc<Database>, config: Arc<Config>) -> Self {
        Self { db, config }
    }

    /// Read a resource
    pub async fn read(&self, uri: &str) -> Result<Value> {
        debug!("Reading resource: {}", uri);

        // Parse the URI
        if let Some(rest) = uri.strip_prefix("email://") {
            self.read_email_resource(rest).await
        } else if let Some(rest) = uri.strip_prefix("calendar://") {
            self.read_calendar_resource(rest).await
        } else {
            Err(Error::ResourceNotFound(uri.to_string()))
        }
    }

    /// Read an email resource
    async fn read_email_resource(&self, path: &str) -> Result<Value> {
        let parts: Vec<&str> = path.split('/').collect();

        match parts.as_slice() {
            [message_id] => {
                // Full email content
                let email = self
                    .db
                    .get_email(message_id)
                    .await?
                    .ok_or_else(|| Error::EmailNotFound(message_id.to_string()))?;

                Ok(serde_json::json!({
                    "contents": [{
                        "uri": format!("email://{}", message_id),
                        "mimeType": "message/rfc822",
                        "text": format!(
                            "From: {}\nTo: {}\nSubject: {}\nDate: {}\n\n{}",
                            email.from,
                            email.to.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "),
                            email.subject,
                            email.date.to_rfc2822(),
                            email.body_plain
                        )
                    }]
                }))
            }
            [message_id, "body"] => {
                // Email body only
                let email = self
                    .db
                    .get_email(message_id)
                    .await?
                    .ok_or_else(|| Error::EmailNotFound(message_id.to_string()))?;

                Ok(serde_json::json!({
                    "contents": [{
                        "uri": format!("email://{}/body", message_id),
                        "mimeType": "text/plain",
                        "text": email.body_plain
                    }]
                }))
            }
            [message_id, "attachments", filename] => {
                // Attachment content
                let email = self
                    .db
                    .get_email(message_id)
                    .await?
                    .ok_or_else(|| Error::EmailNotFound(message_id.to_string()))?;

                let attachment = email
                    .attachments
                    .iter()
                    .find(|a| a.filename == *filename)
                    .ok_or_else(|| {
                        Error::ResourceNotFound(format!("Attachment {} not found", filename))
                    })?;

                // Check if attachment is downloaded
                if let Some(path) = &attachment.local_path {
                    if path.exists() {
                        let content = std::fs::read(path)?;
                        let base64_content = base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &content,
                        );

                        return Ok(serde_json::json!({
                            "contents": [{
                                "uri": format!("email://{}/attachments/{}", message_id, filename),
                                "mimeType": attachment.mime_type,
                                "blob": base64_content
                            }]
                        }));
                    }
                }

                Err(Error::ResourceNotFound(format!(
                    "Attachment {} not downloaded. Use the daemon to download it first.",
                    filename
                )))
            }
            _ => Err(Error::ResourceNotFound(format!("Invalid email URI: {}", path))),
        }
    }

    /// Read a calendar resource
    async fn read_calendar_resource(&self, path: &str) -> Result<Value> {
        let event_id = path.trim_matches('/');

        // TODO: Implement event fetching and iCalendar formatting
        Err(Error::ResourceNotFound(format!(
            "Calendar resource reading not yet implemented: {}",
            event_id
        )))
    }
}
