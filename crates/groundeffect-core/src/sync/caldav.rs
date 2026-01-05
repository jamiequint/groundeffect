//! CalDAV client for Google Calendar

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use reqwest::Client;
use tracing::{debug, error, info};

use crate::error::{Error, Result};
use crate::models::{
    Attendee, AttendeeStatus, CalendarEvent, EventStatus, EventTime, Reminder, ReminderMethod,
    Transparency,
};
use crate::oauth::OAuthManager;

use super::GlobalRateLimiter;

/// Google Calendar CalDAV endpoint
const CALDAV_BASE: &str = "https://apidata.googleusercontent.com/caldav/v2";

/// CalDAV client for a single account
pub struct CalDavClient {
    account_id: String,
    oauth: Arc<OAuthManager>,
    rate_limiter: Arc<GlobalRateLimiter>,
    client: Client,
}

impl CalDavClient {
    /// Create a new CalDAV client
    pub async fn new(
        account_id: &str,
        oauth: Arc<OAuthManager>,
        rate_limiter: Arc<GlobalRateLimiter>,
    ) -> Result<Self> {
        Ok(Self {
            account_id: account_id.to_string(),
            oauth,
            rate_limiter,
            client: Client::new(),
        })
    }

    /// Fetch events from the primary calendar with optional date filter
    pub async fn fetch_events(&self, since: Option<DateTime<Utc>>) -> Result<Vec<CalendarEvent>> {
        self.rate_limiter.wait().await;

        let access_token = self.oauth.get_valid_token(&self.account_id).await?;

        // Default to 1 year ago if no since date specified
        // Google Calendar API requires timeMin when using singleEvents=true with orderBy=startTime
        let time_min = since.unwrap_or_else(|| Utc::now() - chrono::Duration::days(365));
        // Format as RFC3339 and URL-encode (the '+' in timezone needs encoding)
        let time_min_encoded = time_min.to_rfc3339().replace('+', "%2B");

        // Use Google Calendar API instead of CalDAV for easier parsing
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/primary/events?\
             maxResults=2500&\
             singleEvents=true&\
             orderBy=startTime&\
             timeMin={}",
            time_min_encoded
        );

        let response = self
            .client
            .get(&url)
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::CalDav(format!(
                "Failed to fetch events: {} - {}",
                status, body
            )));
        }

        let json: serde_json::Value = response.json().await?;
        let items = json["items"].as_array().ok_or_else(|| {
            Error::CalDav("Invalid response: no items array".to_string())
        })?;

        let mut events = Vec::new();
        for item in items {
            if let Some(event) = self.parse_google_event(item)? {
                events.push(event);
            }
        }

        info!("Fetched {} events for {}", events.len(), self.account_id);
        Ok(events)
    }

    /// Parse a Google Calendar API event into our CalendarEvent struct
    fn parse_google_event(&self, json: &serde_json::Value) -> Result<Option<CalendarEvent>> {
        let id = json["id"].as_str().unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }

        let summary = json["summary"].as_str().unwrap_or("(No Title)").to_string();

        // Parse start time
        let (start, all_day) = if let Some(start_date) = json["start"]["date"].as_str() {
            let date = NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
                .map_err(|e| Error::CalDav(format!("Invalid date: {}", e)))?;
            (EventTime::Date(date), true)
        } else if let Some(start_dt) = json["start"]["dateTime"].as_str() {
            let dt = DateTime::parse_from_rfc3339(start_dt)
                .map_err(|e| Error::CalDav(format!("Invalid datetime: {}", e)))?
                .with_timezone(&Utc);
            (EventTime::DateTime(dt), false)
        } else {
            return Ok(None); // Skip events without start time
        };

        // Parse end time
        let end = if let Some(end_date) = json["end"]["date"].as_str() {
            let date = NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
                .map_err(|e| Error::CalDav(format!("Invalid date: {}", e)))?;
            EventTime::Date(date)
        } else if let Some(end_dt) = json["end"]["dateTime"].as_str() {
            let dt = DateTime::parse_from_rfc3339(end_dt)
                .map_err(|e| Error::CalDav(format!("Invalid datetime: {}", e)))?
                .with_timezone(&Utc);
            EventTime::DateTime(dt)
        } else {
            start.clone()
        };

        let timezone = json["start"]["timeZone"]
            .as_str()
            .unwrap_or("UTC")
            .to_string();

        // Parse organizer
        let organizer = json["organizer"].as_object().map(|org| Attendee {
            email: org.get("email").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            name: org.get("displayName").and_then(|v| v.as_str()).map(|s| s.to_string()),
            response_status: Some(AttendeeStatus::Accepted),
            optional: false,
        });

        // Parse attendees
        let attendees: Vec<Attendee> = json["attendees"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|att| {
                        Some(Attendee {
                            email: att["email"].as_str()?.to_string(),
                            name: att["displayName"].as_str().map(|s| s.to_string()),
                            response_status: att["responseStatus"].as_str().and_then(|s| {
                                match s {
                                    "needsAction" => Some(AttendeeStatus::NeedsAction),
                                    "declined" => Some(AttendeeStatus::Declined),
                                    "tentative" => Some(AttendeeStatus::Tentative),
                                    "accepted" => Some(AttendeeStatus::Accepted),
                                    _ => None,
                                }
                            }),
                            optional: att["optional"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Parse status
        let status = match json["status"].as_str() {
            Some("confirmed") => EventStatus::Confirmed,
            Some("tentative") => EventStatus::Tentative,
            Some("cancelled") => EventStatus::Cancelled,
            _ => EventStatus::Confirmed,
        };

        // Parse transparency
        let transparency = match json["transparency"].as_str() {
            Some("transparent") => Transparency::Transparent,
            _ => Transparency::Opaque,
        };

        // Parse reminders
        let reminders: Vec<Reminder> = json["reminders"]["overrides"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        Some(Reminder {
                            method: match r["method"].as_str()? {
                                "popup" => ReminderMethod::Popup,
                                "email" => ReminderMethod::Email,
                                _ => return None,
                            },
                            minutes: r["minutes"].as_i64()? as i32,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let event = CalendarEvent {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: self.account_id.clone(),
            account_alias: None,
            google_event_id: id.to_string(),
            ical_uid: json["iCalUID"].as_str().unwrap_or(id).to_string(),
            etag: json["etag"].as_str().unwrap_or_default().to_string(),
            summary,
            description: json["description"].as_str().map(|s| s.to_string()),
            location: json["location"].as_str().map(|s| s.to_string()),
            start,
            end,
            timezone,
            all_day,
            recurrence_rule: json["recurrence"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            recurrence_id: json["recurringEventId"].as_str().map(|s| s.to_string()),
            organizer,
            attendees,
            status,
            transparency,
            reminders,
            embedding: None,
            calendar_id: "primary".to_string(),
            synced_at: Utc::now(),
        };

        Ok(Some(event))
    }

    /// Create a new calendar event
    pub async fn create_event(&self, event: &CalendarEvent) -> Result<String> {
        self.rate_limiter.wait().await;

        let access_token = self.oauth.get_valid_token(&self.account_id).await?;

        let calendar_id = &event.calendar_id;
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            calendar_id
        );

        let body = self.event_to_google_json(event)?;

        let response = self
            .client
            .post(&url)
            .bearer_auth(&access_token)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::CalDav(format!(
                "Failed to create event: {} - {}",
                status, body
            )));
        }

        let json: serde_json::Value = response.json().await?;
        let event_id = json["id"].as_str().unwrap_or_default().to_string();

        info!("Created event {} for {}", event_id, self.account_id);
        Ok(event_id)
    }

    /// Update an existing calendar event
    pub async fn update_event(&self, event: &CalendarEvent) -> Result<()> {
        self.rate_limiter.wait().await;

        let access_token = self.oauth.get_valid_token(&self.account_id).await?;

        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            event.calendar_id, event.google_event_id
        );

        let body = self.event_to_google_json(event)?;

        let response = self
            .client
            .put(&url)
            .bearer_auth(&access_token)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::CalDav(format!(
                "Failed to update event: {} - {}",
                status, body
            )));
        }

        info!("Updated event {} for {}", event.google_event_id, self.account_id);
        Ok(())
    }

    /// Delete a calendar event
    pub async fn delete_event(&self, calendar_id: &str, event_id: &str) -> Result<()> {
        self.rate_limiter.wait().await;

        let access_token = self.oauth.get_valid_token(&self.account_id).await?;

        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            calendar_id, event_id
        );

        let response = self
            .client
            .delete(&url)
            .bearer_auth(&access_token)
            .send()
            .await?;

        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::CalDav(format!(
                "Failed to delete event: {} - {}",
                status, body
            )));
        }

        info!("Deleted event {} for {}", event_id, self.account_id);
        Ok(())
    }

    /// Convert our event to Google Calendar API JSON
    fn event_to_google_json(&self, event: &CalendarEvent) -> Result<serde_json::Value> {
        let start = match &event.start {
            EventTime::DateTime(dt) => serde_json::json!({
                "dateTime": dt.to_rfc3339(),
                "timeZone": event.timezone
            }),
            EventTime::Date(d) => serde_json::json!({
                "date": d.to_string()
            }),
        };

        let end = match &event.end {
            EventTime::DateTime(dt) => serde_json::json!({
                "dateTime": dt.to_rfc3339(),
                "timeZone": event.timezone
            }),
            EventTime::Date(d) => serde_json::json!({
                "date": d.to_string()
            }),
        };

        let mut json = serde_json::json!({
            "summary": event.summary,
            "start": start,
            "end": end
        });

        if let Some(desc) = &event.description {
            json["description"] = serde_json::Value::String(desc.clone());
        }

        if let Some(loc) = &event.location {
            json["location"] = serde_json::Value::String(loc.clone());
        }

        if !event.attendees.is_empty() {
            json["attendees"] = serde_json::json!(
                event.attendees.iter().map(|a| {
                    serde_json::json!({
                        "email": a.email,
                        "optional": a.optional
                    })
                }).collect::<Vec<_>>()
            );
        }

        Ok(json)
    }
}
