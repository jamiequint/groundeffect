//! Calendar event data structures

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Event time - can be a specific datetime or an all-day date
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventTime {
    DateTime(DateTime<Utc>),
    Date(NaiveDate),
}

impl EventTime {
    /// Check if this is an all-day event time
    pub fn is_all_day(&self) -> bool {
        matches!(self, EventTime::Date(_))
    }

    /// Get as DateTime if applicable
    pub fn as_datetime(&self) -> Option<DateTime<Utc>> {
        match self {
            EventTime::DateTime(dt) => Some(*dt),
            EventTime::Date(_) => None,
        }
    }

    /// Get as NaiveDate
    pub fn as_date(&self) -> NaiveDate {
        match self {
            EventTime::DateTime(dt) => dt.date_naive(),
            EventTime::Date(d) => *d,
        }
    }
}

/// Event status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}

impl Default for EventStatus {
    fn default() -> Self {
        Self::Confirmed
    }
}

/// Event transparency (free/busy)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transparency {
    /// Blocks time (busy)
    Opaque,
    /// Doesn't block time (free)
    Transparent,
}

impl Default for Transparency {
    fn default() -> Self {
        Self::Opaque
    }
}

/// Event attendee
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attendee {
    /// Email address
    pub email: String,

    /// Display name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Response status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_status: Option<AttendeeStatus>,

    /// Whether this attendee is optional
    #[serde(default)]
    pub optional: bool,
}

/// Attendee response status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttendeeStatus {
    NeedsAction,
    Declined,
    Tentative,
    Accepted,
}

/// Event reminder
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    /// Reminder method
    pub method: ReminderMethod,

    /// Minutes before event
    pub minutes: i32,
}

/// Reminder delivery method
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderMethod {
    Popup,
    Email,
}

/// A calendar event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    // === Identifiers ===
    /// Internal UUID
    pub id: String,

    /// Account identifier (email address)
    pub account_id: String,

    /// User-defined account alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_alias: Option<String>,

    /// Google Calendar event ID
    pub google_event_id: String,

    /// iCalendar UID
    pub ical_uid: String,

    /// ETag for change detection
    pub etag: String,

    // === Event data ===
    /// Event title/summary
    pub summary: String,

    /// Event description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Event location
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,

    // === Timing ===
    /// Start time
    pub start: EventTime,

    /// End time
    pub end: EventTime,

    /// Timezone
    pub timezone: String,

    /// Whether this is an all-day event
    pub all_day: bool,

    // === Recurrence ===
    /// Recurrence rule (RRULE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<String>,

    /// Recurrence ID (for exceptions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,

    // === Attendees ===
    /// Event organizer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Attendee>,

    /// Event attendees
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,

    // === Status ===
    /// Event status
    #[serde(default)]
    pub status: EventStatus,

    /// Free/busy transparency
    #[serde(default)]
    pub transparency: Transparency,

    // === Reminders ===
    /// Event reminders
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reminders: Vec<Reminder>,

    // === Search ===
    /// Embedding vector (768 dimensions)
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,

    // === Sync metadata ===
    /// Calendar ID this event belongs to
    pub calendar_id: String,

    /// When this event was last synced
    pub synced_at: DateTime<Utc>,
}

impl CalendarEvent {
    /// Get searchable text for embedding
    pub fn searchable_text(&self) -> String {
        let mut text = String::new();

        // Summary (weighted by repetition for importance)
        text.push_str(&self.summary);
        text.push_str(". ");
        text.push_str(&self.summary);
        text.push_str(". ");

        // Description
        if let Some(desc) = &self.description {
            text.push_str(desc);
            text.push_str(". ");
        }

        // Location
        if let Some(loc) = &self.location {
            text.push_str("Location: ");
            text.push_str(loc);
            text.push_str(". ");
        }

        // Attendees
        if !self.attendees.is_empty() {
            text.push_str("Attendees: ");
            for attendee in &self.attendees {
                if let Some(name) = &attendee.name {
                    text.push_str(name);
                } else {
                    text.push_str(&attendee.email);
                }
                text.push_str(", ");
            }
        }

        text
    }

    /// Generate a markdown summary
    pub fn markdown_summary(&self) -> String {
        let account_display = match &self.account_alias {
            Some(alias) => format!("{} ({})", self.account_id, alias),
            None => self.account_id.clone(),
        };

        let time_str = match (&self.start, &self.end) {
            (EventTime::Date(start), EventTime::Date(end)) if start == end => {
                format!("{} (all day)", start.format("%b %d, %Y"))
            }
            (EventTime::Date(start), EventTime::Date(end)) => {
                format!(
                    "{} - {} (all day)",
                    start.format("%b %d, %Y"),
                    end.format("%b %d, %Y")
                )
            }
            (EventTime::DateTime(start), EventTime::DateTime(end)) => {
                if start.date_naive() == end.date_naive() {
                    format!(
                        "{} {} - {}",
                        start.format("%b %d, %Y"),
                        start.format("%I:%M %p"),
                        end.format("%I:%M %p")
                    )
                } else {
                    format!(
                        "{} - {}",
                        start.format("%b %d, %Y %I:%M %p"),
                        end.format("%b %d, %Y %I:%M %p")
                    )
                }
            }
            _ => String::from("Time TBD"),
        };

        let mut summary = format!(
            "**Account:** {}\n**Event:** {}\n**When:** {}",
            account_display, self.summary, time_str
        );

        if let Some(loc) = &self.location {
            summary.push_str(&format!("\n**Where:** {}", loc));
        }

        if let Some(desc) = &self.description {
            let truncated = if desc.len() > 200 {
                format!("{}...", &desc[..200])
            } else {
                desc.clone()
            };
            summary.push_str(&format!("\n\n{}", truncated));
        }

        summary
    }
}

/// Calendar event search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSearchResult {
    /// The event data
    #[serde(flatten)]
    pub event: EventSummary,

    /// Combined search score (RRF)
    pub score: f32,

    /// Markdown summary for LLM consumption
    pub markdown_summary: String,
}

/// Lightweight event summary for search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub id: String,
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_alias: Option<String>,
    pub google_event_id: String,
    pub summary: String,
    pub start: EventTime,
    pub end: EventTime,
    pub all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub calendar_id: String,
    pub status: EventStatus,
}

impl From<&CalendarEvent> for EventSummary {
    fn from(event: &CalendarEvent) -> Self {
        Self {
            id: event.id.clone(),
            account_id: event.account_id.clone(),
            account_alias: event.account_alias.clone(),
            google_event_id: event.google_event_id.clone(),
            summary: event.summary.clone(),
            start: event.start.clone(),
            end: event.end.clone(),
            all_day: event.all_day,
            location: event.location.clone(),
            calendar_id: event.calendar_id.clone(),
            status: event.status.clone(),
        }
    }
}

/// Request to create a calendar event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEventRequest {
    /// Account to create event on (email or alias)
    pub account: String,

    /// Calendar ID (omit for primary calendar)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,

    /// Event title
    pub summary: String,

    /// Start time (ISO 8601)
    pub start: String,

    /// End time (ISO 8601)
    pub end: String,

    /// Event description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Event location
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,

    /// Attendee email addresses
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,

    /// Reminders
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reminders: Vec<Reminder>,
}

/// A calendar (container for events)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Calendar {
    /// Calendar ID
    pub id: String,

    /// Account ID this calendar belongs to
    pub account_id: String,

    /// Calendar name
    pub name: String,

    /// Calendar description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether this is the primary calendar
    pub primary: bool,

    /// Calendar color (hex)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Calendar timezone
    pub timezone: String,
}
