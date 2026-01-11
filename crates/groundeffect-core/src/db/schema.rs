//! LanceDB schema definitions and conversion functions

use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, BooleanArray, FixedSizeListArray, Float32Array, Int64Array, RecordBatch,
    StringArray, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use chrono::{DateTime, Utc};
use lance_arrow::FixedSizeListArrayExt;

use crate::error::{Error, Result};
use crate::models::{
    Account, AccountStatus, Address, Attendee, CalendarEvent, Email, EventStatus, EventTime,
    Reminder, Transparency,
};
use crate::EMBEDDING_DIMENSION;

/// Create the emails table schema
pub fn email_schema() -> Schema {
    Schema::new(vec![
        // Identifiers
        Field::new("id", DataType::Utf8, false),
        Field::new("account_id", DataType::Utf8, false),
        Field::new("account_alias", DataType::Utf8, true),
        Field::new("message_id", DataType::Utf8, false),
        Field::new("gmail_message_id", DataType::UInt64, false),
        Field::new("gmail_thread_id", DataType::UInt64, false),
        Field::new("uid", DataType::UInt32, false),
        // Threading
        Field::new("in_reply_to", DataType::Utf8, true),
        Field::new("references", DataType::Utf8, true), // JSON array
        // Metadata
        Field::new("folder", DataType::Utf8, false),
        Field::new("labels", DataType::Utf8, true),  // JSON array
        Field::new("flags", DataType::Utf8, true),   // JSON array
        // Headers
        Field::new("from_email", DataType::Utf8, false),
        Field::new("from_name", DataType::Utf8, true),
        Field::new("to", DataType::Utf8, true),      // JSON array
        Field::new("cc", DataType::Utf8, true),      // JSON array
        Field::new("bcc", DataType::Utf8, true),     // JSON array
        Field::new("subject", DataType::Utf8, false),
        Field::new("date", DataType::Int64, false),  // Unix timestamp
        // Content
        Field::new("body_plain", DataType::Utf8, false),
        Field::new("body_html", DataType::Utf8, true),
        Field::new("snippet", DataType::Utf8, false),
        // Attachments (JSON array)
        Field::new("attachments", DataType::Utf8, true),
        // Embedding vector
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIMENSION as i32,
            ),
            true,
        ),
        // Sync metadata
        Field::new("synced_at", DataType::Int64, false),
        Field::new("raw_size", DataType::UInt64, false),
    ])
}

/// Create the events table schema
pub fn event_schema() -> Schema {
    Schema::new(vec![
        // Identifiers
        Field::new("id", DataType::Utf8, false),
        Field::new("account_id", DataType::Utf8, false),
        Field::new("account_alias", DataType::Utf8, true),
        Field::new("google_event_id", DataType::Utf8, false),
        Field::new("ical_uid", DataType::Utf8, false),
        Field::new("etag", DataType::Utf8, false),
        // Event data
        Field::new("summary", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, true),
        Field::new("location", DataType::Utf8, true),
        // Timing
        Field::new("start", DataType::Utf8, false),  // ISO 8601 or date
        Field::new("end", DataType::Utf8, false),    // ISO 8601 or date
        Field::new("timezone", DataType::Utf8, false),
        Field::new("all_day", DataType::UInt32, false), // 0 or 1
        // Recurrence
        Field::new("recurrence_rule", DataType::Utf8, true),
        Field::new("recurrence_id", DataType::Utf8, true),
        // Attendees (JSON)
        Field::new("organizer", DataType::Utf8, true),
        Field::new("attendees", DataType::Utf8, true),
        // Status
        Field::new("status", DataType::Utf8, false),
        Field::new("transparency", DataType::Utf8, false),
        // Reminders (JSON array)
        Field::new("reminders", DataType::Utf8, true),
        // Embedding vector
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIMENSION as i32,
            ),
            true,
        ),
        // Sync metadata
        Field::new("calendar_id", DataType::Utf8, false),
        Field::new("synced_at", DataType::Int64, false),
    ])
}

/// Create the accounts table schema
pub fn account_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("alias", DataType::Utf8, true),
        Field::new("display_name", DataType::Utf8, false),
        Field::new("added_at", DataType::Int64, false),
        Field::new("last_sync_email", DataType::Int64, true),
        Field::new("last_sync_calendar", DataType::Int64, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("sync_email_since", DataType::Int64, true),
        Field::new("oldest_email_synced", DataType::Int64, true),
        Field::new("oldest_event_synced", DataType::Int64, true),
        Field::new("sync_attachments", DataType::Boolean, false),
        Field::new("estimated_total_emails", DataType::Int64, true),
    ])
}

/// Create an empty batch for the emails schema
pub fn empty_email_batch(schema: &Schema) -> RecordBatch {
    let arrays: Vec<ArrayRef> = schema
        .fields()
        .iter()
        .map(|field| match field.data_type() {
            DataType::Utf8 => Arc::new(StringArray::from(Vec::<Option<&str>>::new())) as ArrayRef,
            DataType::UInt32 => Arc::new(UInt32Array::from(Vec::<u32>::new())) as ArrayRef,
            DataType::UInt64 => Arc::new(UInt64Array::from(Vec::<u64>::new())) as ArrayRef,
            DataType::Int64 => Arc::new(Int64Array::from(Vec::<i64>::new())) as ArrayRef,
            DataType::FixedSizeList(_, size) => {
                let values = Float32Array::from(Vec::<f32>::new());
                Arc::new(
                    FixedSizeListArray::try_new_from_values(values, *size).unwrap(),
                ) as ArrayRef
            }
            _ => panic!("Unsupported type: {:?}", field.data_type()),
        })
        .collect();

    RecordBatch::try_new(Arc::new(schema.clone()), arrays).unwrap()
}

/// Create an empty batch for the events schema
pub fn empty_event_batch(schema: &Schema) -> RecordBatch {
    empty_email_batch(schema) // Same logic
}

/// Create an empty batch for the accounts schema
pub fn empty_account_batch(schema: &Schema) -> RecordBatch {
    let arrays: Vec<ArrayRef> = schema
        .fields()
        .iter()
        .map(|field| match field.data_type() {
            DataType::Utf8 => Arc::new(StringArray::from(Vec::<Option<&str>>::new())) as ArrayRef,
            DataType::Int64 => Arc::new(Int64Array::from(Vec::<i64>::new())) as ArrayRef,
            DataType::Boolean => Arc::new(BooleanArray::from(Vec::<bool>::new())) as ArrayRef,
            _ => panic!("Unsupported type: {:?}", field.data_type()),
        })
        .collect();

    RecordBatch::try_new(Arc::new(schema.clone()), arrays).unwrap()
}

/// Convert an email to a record batch
pub fn email_to_batch(email: &Email) -> Result<RecordBatch> {
    emails_to_batch(&[email.clone()])
}

/// Convert multiple emails to a record batch
pub fn emails_to_batch(emails: &[Email]) -> Result<RecordBatch> {
    let schema = email_schema();

    let ids: Vec<&str> = emails.iter().map(|e| e.id.as_str()).collect();
    let account_ids: Vec<&str> = emails.iter().map(|e| e.account_id.as_str()).collect();
    let account_aliases: Vec<Option<&str>> = emails
        .iter()
        .map(|e| e.account_alias.as_deref())
        .collect();
    let message_ids: Vec<&str> = emails.iter().map(|e| e.message_id.as_str()).collect();
    let gmail_message_ids: Vec<u64> = emails.iter().map(|e| e.gmail_message_id).collect();
    let gmail_thread_ids: Vec<u64> = emails.iter().map(|e| e.gmail_thread_id).collect();
    let uids: Vec<u32> = emails.iter().map(|e| e.uid).collect();
    let in_reply_tos: Vec<Option<&str>> = emails.iter().map(|e| e.in_reply_to.as_deref()).collect();
    let references: Vec<Option<String>> = emails
        .iter()
        .map(|e| {
            if e.references.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&e.references).unwrap())
            }
        })
        .collect();
    let folders: Vec<&str> = emails.iter().map(|e| e.folder.as_str()).collect();
    let labels: Vec<Option<String>> = emails
        .iter()
        .map(|e| {
            if e.labels.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&e.labels).unwrap())
            }
        })
        .collect();
    let flags: Vec<Option<String>> = emails
        .iter()
        .map(|e| {
            if e.flags.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&e.flags).unwrap())
            }
        })
        .collect();
    let from_emails: Vec<&str> = emails.iter().map(|e| e.from.email.as_str()).collect();
    let from_names: Vec<Option<&str>> = emails
        .iter()
        .map(|e| e.from.name.as_deref())
        .collect();
    let tos: Vec<Option<String>> = emails
        .iter()
        .map(|e| Some(serde_json::to_string(&e.to).unwrap()))
        .collect();
    let ccs: Vec<Option<String>> = emails
        .iter()
        .map(|e| {
            if e.cc.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&e.cc).unwrap())
            }
        })
        .collect();
    let bccs: Vec<Option<String>> = emails
        .iter()
        .map(|e| {
            if e.bcc.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&e.bcc).unwrap())
            }
        })
        .collect();
    let subjects: Vec<&str> = emails.iter().map(|e| e.subject.as_str()).collect();
    let dates: Vec<i64> = emails.iter().map(|e| e.date.timestamp()).collect();
    let body_plains: Vec<&str> = emails.iter().map(|e| e.body_plain.as_str()).collect();
    let body_htmls: Vec<Option<&str>> = emails.iter().map(|e| e.body_html.as_deref()).collect();
    let snippets: Vec<&str> = emails.iter().map(|e| e.snippet.as_str()).collect();
    let attachments: Vec<Option<String>> = emails
        .iter()
        .map(|e| {
            if e.attachments.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&e.attachments).unwrap())
            }
        })
        .collect();

    // Build embedding array
    let embedding_values: Vec<f32> = emails
        .iter()
        .flat_map(|e| {
            e.embedding
                .as_ref()
                .map(|v| v.clone())
                .unwrap_or_else(|| vec![0.0; EMBEDDING_DIMENSION])
        })
        .collect();
    let embedding_array = FixedSizeListArray::try_new_from_values(
        Float32Array::from(embedding_values),
        EMBEDDING_DIMENSION as i32,
    )
    .map_err(|e| Error::Arrow(e))?;

    let synced_ats: Vec<i64> = emails.iter().map(|e| e.synced_at.timestamp()).collect();
    let raw_sizes: Vec<u64> = emails.iter().map(|e| e.raw_size).collect();

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(ids)),
        Arc::new(StringArray::from(account_ids)),
        Arc::new(StringArray::from(account_aliases)),
        Arc::new(StringArray::from(message_ids)),
        Arc::new(UInt64Array::from(gmail_message_ids)),
        Arc::new(UInt64Array::from(gmail_thread_ids)),
        Arc::new(UInt32Array::from(uids)),
        Arc::new(StringArray::from(in_reply_tos)),
        Arc::new(StringArray::from(
            references.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(folders)),
        Arc::new(StringArray::from(
            labels.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            flags.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(from_emails)),
        Arc::new(StringArray::from(from_names)),
        Arc::new(StringArray::from(
            tos.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            ccs.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            bccs.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(subjects)),
        Arc::new(Int64Array::from(dates)),
        Arc::new(StringArray::from(body_plains)),
        Arc::new(StringArray::from(body_htmls)),
        Arc::new(StringArray::from(snippets)),
        Arc::new(StringArray::from(
            attachments.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(embedding_array),
        Arc::new(Int64Array::from(synced_ats)),
        Arc::new(UInt64Array::from(raw_sizes)),
    ];

    let batch = RecordBatch::try_new(Arc::new(schema), arrays)?;
    Ok(batch)
}

/// Convert a record batch row to an email
pub fn batch_to_email(batch: &RecordBatch, row: usize) -> Result<Email> {
    let get_string = |col: &str| -> String {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| a.value(row).to_string().into())
            .unwrap_or_default()
    };

    let get_opt_string = |col: &str| -> Option<String> {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row).to_string())
                }
            })
    };

    let get_u64 = |col: &str| -> u64 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    let get_u32 = |col: &str| -> u32 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    let get_i64 = |col: &str| -> i64 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    let references: Vec<String> = get_opt_string("references")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let labels: Vec<String> = get_opt_string("labels")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let flags: Vec<String> = get_opt_string("flags")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let to: Vec<Address> = get_opt_string("to")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let cc: Vec<Address> = get_opt_string("cc")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let bcc: Vec<Address> = get_opt_string("bcc")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let attachments = get_opt_string("attachments")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let date = DateTime::from_timestamp(get_i64("date"), 0).unwrap_or_else(Utc::now);
    let synced_at = DateTime::from_timestamp(get_i64("synced_at"), 0).unwrap_or_else(Utc::now);

    Ok(Email {
        id: get_string("id"),
        account_id: get_string("account_id"),
        account_alias: get_opt_string("account_alias"),
        message_id: get_string("message_id"),
        gmail_message_id: get_u64("gmail_message_id"),
        gmail_thread_id: get_u64("gmail_thread_id"),
        uid: get_u32("uid"),
        in_reply_to: get_opt_string("in_reply_to"),
        references,
        folder: get_string("folder"),
        labels,
        flags,
        from: Address {
            email: get_string("from_email"),
            name: get_opt_string("from_name"),
        },
        to,
        cc,
        bcc,
        subject: get_string("subject"),
        date,
        body_plain: get_string("body_plain"),
        body_html: get_opt_string("body_html"),
        snippet: get_string("snippet"),
        attachments,
        embedding: None, // Don't load embedding by default
        synced_at,
        raw_size: get_u64("raw_size"),
    })
}

/// Convert a single event to a record batch
pub fn event_to_batch(event: &CalendarEvent) -> Result<RecordBatch> {
    events_to_batch(&[event.clone()])
}

/// Convert multiple events to a record batch
pub fn events_to_batch(events: &[CalendarEvent]) -> Result<RecordBatch> {
    let schema = event_schema();

    let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
    let account_ids: Vec<&str> = events.iter().map(|e| e.account_id.as_str()).collect();
    let account_aliases: Vec<Option<&str>> = events.iter().map(|e| e.account_alias.as_deref()).collect();
    let google_event_ids: Vec<&str> = events.iter().map(|e| e.google_event_id.as_str()).collect();
    let ical_uids: Vec<&str> = events.iter().map(|e| e.ical_uid.as_str()).collect();
    let etags: Vec<&str> = events.iter().map(|e| e.etag.as_str()).collect();
    let summaries: Vec<&str> = events.iter().map(|e| e.summary.as_str()).collect();
    let descriptions: Vec<Option<&str>> = events.iter().map(|e| e.description.as_deref()).collect();
    let locations: Vec<Option<&str>> = events.iter().map(|e| e.location.as_deref()).collect();

    let starts: Vec<String> = events.iter().map(|e| match &e.start {
        EventTime::DateTime(dt) => dt.to_rfc3339(),
        EventTime::Date(d) => d.to_string(),
    }).collect();
    let ends: Vec<String> = events.iter().map(|e| match &e.end {
        EventTime::DateTime(dt) => dt.to_rfc3339(),
        EventTime::Date(d) => d.to_string(),
    }).collect();

    let timezones: Vec<&str> = events.iter().map(|e| e.timezone.as_str()).collect();
    let all_days: Vec<u32> = events.iter().map(|e| if e.all_day { 1u32 } else { 0u32 }).collect();
    let recurrence_rules: Vec<Option<&str>> = events.iter().map(|e| e.recurrence_rule.as_deref()).collect();
    let recurrence_ids: Vec<Option<&str>> = events.iter().map(|e| e.recurrence_id.as_deref()).collect();

    let organizers: Vec<Option<String>> = events.iter().map(|e| {
        e.organizer.as_ref().map(|o| serde_json::to_string(o).unwrap())
    }).collect();
    let attendees: Vec<Option<String>> = events.iter().map(|e| {
        if e.attendees.is_empty() { None } else { Some(serde_json::to_string(&e.attendees).unwrap()) }
    }).collect();

    let statuses: Vec<String> = events.iter().map(|e| {
        serde_json::to_string(&e.status).unwrap().trim_matches('"').to_string()
    }).collect();
    let transparencies: Vec<String> = events.iter().map(|e| {
        serde_json::to_string(&e.transparency).unwrap().trim_matches('"').to_string()
    }).collect();

    let reminders: Vec<Option<String>> = events.iter().map(|e| {
        if e.reminders.is_empty() { None } else { Some(serde_json::to_string(&e.reminders).unwrap()) }
    }).collect();

    // Build embedding array
    let embedding_values: Vec<f32> = events
        .iter()
        .flat_map(|e| {
            e.embedding
                .as_ref()
                .cloned()
                .unwrap_or_else(|| vec![0.0; EMBEDDING_DIMENSION])
        })
        .collect();
    let embedding_array = FixedSizeListArray::try_new_from_values(
        Float32Array::from(embedding_values),
        EMBEDDING_DIMENSION as i32,
    )?;

    let calendar_ids: Vec<&str> = events.iter().map(|e| e.calendar_id.as_str()).collect();
    let synced_ats: Vec<i64> = events.iter().map(|e| e.synced_at.timestamp()).collect();

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(ids)),
        Arc::new(StringArray::from(account_ids)),
        Arc::new(StringArray::from(account_aliases)),
        Arc::new(StringArray::from(google_event_ids)),
        Arc::new(StringArray::from(ical_uids)),
        Arc::new(StringArray::from(etags)),
        Arc::new(StringArray::from(summaries)),
        Arc::new(StringArray::from(descriptions)),
        Arc::new(StringArray::from(locations)),
        Arc::new(StringArray::from(starts.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(ends.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(timezones)),
        Arc::new(UInt32Array::from(all_days)),
        Arc::new(StringArray::from(recurrence_rules)),
        Arc::new(StringArray::from(recurrence_ids)),
        Arc::new(StringArray::from(organizers.iter().map(|s| s.as_deref()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(attendees.iter().map(|s| s.as_deref()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(statuses.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(transparencies.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(reminders.iter().map(|s| s.as_deref()).collect::<Vec<_>>())),
        Arc::new(embedding_array),
        Arc::new(StringArray::from(calendar_ids)),
        Arc::new(Int64Array::from(synced_ats)),
    ];

    let batch = RecordBatch::try_new(Arc::new(schema), arrays)?;
    Ok(batch)
}

/// Convert a record batch row to a calendar event
pub fn batch_to_event(batch: &RecordBatch, row: usize) -> Result<CalendarEvent> {
    let get_string = |col: &str| -> String {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(row).to_string())
            .unwrap_or_default()
    };

    let get_optional_string = |col: &str| -> Option<String> {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row).to_string())
                }
            })
    };

    let get_u32 = |col: &str| -> u32 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    let get_i64 = |col: &str| -> i64 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    // Parse start time
    let start_str = get_string("start");
    let start = if start_str.contains('T') {
        // DateTime
        EventTime::DateTime(
            chrono::DateTime::parse_from_rfc3339(&start_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
        )
    } else {
        // Date only
        EventTime::Date(
            chrono::NaiveDate::parse_from_str(&start_str, "%Y-%m-%d")
                .unwrap_or_else(|_| chrono::Utc::now().date_naive()),
        )
    };

    // Parse end time
    let end_str = get_string("end");
    let end = if end_str.contains('T') {
        EventTime::DateTime(
            chrono::DateTime::parse_from_rfc3339(&end_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
        )
    } else {
        EventTime::Date(
            chrono::NaiveDate::parse_from_str(&end_str, "%Y-%m-%d")
                .unwrap_or_else(|_| chrono::Utc::now().date_naive()),
        )
    };

    // Parse organizer JSON
    let organizer: Option<Attendee> = get_optional_string("organizer")
        .and_then(|s| serde_json::from_str(&s).ok());

    // Parse attendees JSON
    let attendees: Vec<Attendee> = get_optional_string("attendees")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // Parse status
    let status_str = get_string("status");
    let status = match status_str.as_str() {
        "Confirmed" | "confirmed" => EventStatus::Confirmed,
        "Tentative" | "tentative" => EventStatus::Tentative,
        "Cancelled" | "cancelled" => EventStatus::Cancelled,
        _ => EventStatus::Confirmed,
    };

    // Parse transparency
    let transparency_str = get_string("transparency");
    let transparency = match transparency_str.as_str() {
        "Transparent" | "transparent" => Transparency::Transparent,
        _ => Transparency::Opaque,
    };

    // Parse reminders JSON
    let reminders: Vec<Reminder> = get_optional_string("reminders")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let synced_at = chrono::DateTime::from_timestamp(get_i64("synced_at"), 0)
        .unwrap_or_else(chrono::Utc::now);

    Ok(CalendarEvent {
        id: get_string("id"),
        account_id: get_string("account_id"),
        account_alias: get_optional_string("account_alias"),
        google_event_id: get_string("google_event_id"),
        ical_uid: get_string("ical_uid"),
        etag: get_string("etag"),
        summary: get_string("summary"),
        description: get_optional_string("description"),
        location: get_optional_string("location"),
        start,
        end,
        timezone: get_string("timezone"),
        all_day: get_u32("all_day") == 1,
        recurrence_rule: get_optional_string("recurrence_rule"),
        recurrence_id: get_optional_string("recurrence_id"),
        organizer,
        attendees,
        status,
        transparency,
        reminders,
        embedding: None, // Don't load embedding by default
        calendar_id: get_string("calendar_id"),
        synced_at,
    })
}

/// Convert an account to a record batch
pub fn account_to_batch(account: &Account) -> Result<RecordBatch> {
    let schema = account_schema();

    let status_str = match &account.status {
        AccountStatus::Active => "active",
        AccountStatus::NeedsReauth => "needs_reauth",
        AccountStatus::Disabled => "disabled",
        AccountStatus::Syncing => "syncing",
    };

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![account.id.as_str()])),
        Arc::new(StringArray::from(vec![account.alias.as_deref()])),
        Arc::new(StringArray::from(vec![account.display_name.as_str()])),
        Arc::new(Int64Array::from(vec![account.added_at.timestamp()])),
        Arc::new(Int64Array::from(vec![account
            .last_sync_email
            .map(|d| d.timestamp())])),
        Arc::new(Int64Array::from(vec![account
            .last_sync_calendar
            .map(|d| d.timestamp())])),
        Arc::new(StringArray::from(vec![status_str])),
        Arc::new(Int64Array::from(vec![account
            .sync_email_since
            .map(|d| d.timestamp())])),
        Arc::new(Int64Array::from(vec![account
            .oldest_email_synced
            .map(|d| d.timestamp())])),
        Arc::new(Int64Array::from(vec![account
            .oldest_event_synced
            .map(|d| d.timestamp())])),
        Arc::new(BooleanArray::from(vec![account.sync_attachments])),
        Arc::new(Int64Array::from(vec![account
            .estimated_total_emails
            .map(|v| v as i64)])),
    ];

    let batch = RecordBatch::try_new(Arc::new(schema), arrays)?;
    Ok(batch)
}

/// Convert a record batch row to an account (lenient version for schema migration)
/// Handles missing columns by using defaults
pub fn batch_to_account_lenient(batch: &RecordBatch, row: usize) -> Result<Account> {
    let get_string = |col: &str| -> String {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(row).to_string())
            .unwrap_or_default()
    };

    let get_opt_string = |col: &str| -> Option<String> {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row).to_string())
                }
            })
    };

    let get_i64 = |col: &str| -> i64 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    let get_opt_i64 = |col: &str| -> Option<i64> {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row))
                }
            })
    };

    let get_bool = |col: &str| -> bool {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .map(|a| a.value(row))
            .unwrap_or(false)
    };

    let status = match get_string("status").as_str() {
        "active" => AccountStatus::Active,
        "needs_reauth" => AccountStatus::NeedsReauth,
        "disabled" => AccountStatus::Disabled,
        "syncing" => AccountStatus::Syncing,
        _ => AccountStatus::Active,
    };

    let added_at = DateTime::from_timestamp(get_i64("added_at"), 0).unwrap_or_else(Utc::now);
    let last_sync_email = get_opt_i64("last_sync_email").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let last_sync_calendar = get_opt_i64("last_sync_calendar").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let sync_email_since = get_opt_i64("sync_email_since").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let oldest_email_synced = get_opt_i64("oldest_email_synced").and_then(|ts| DateTime::from_timestamp(ts, 0));
    // This column may not exist in old schema - defaults to None
    let oldest_event_synced = get_opt_i64("oldest_event_synced").and_then(|ts| DateTime::from_timestamp(ts, 0));
    // This column may not exist in old schema - defaults to false
    let sync_attachments = get_bool("sync_attachments");
    // This column may not exist in old schema - defaults to None
    let estimated_total_emails = get_opt_i64("estimated_total_emails").map(|v| v as u64);

    Ok(Account {
        id: get_string("id"),
        alias: get_opt_string("alias"),
        display_name: get_string("display_name"),
        added_at,
        last_sync_email,
        last_sync_calendar,
        status,
        sync_email_since,
        oldest_email_synced,
        oldest_event_synced,
        sync_attachments,
        estimated_total_emails,
    })
}

/// Convert a record batch row to an account
pub fn batch_to_account(batch: &RecordBatch, row: usize) -> Result<Account> {
    let get_string = |col: &str| -> String {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(row).to_string())
            .unwrap_or_default()
    };

    let get_opt_string = |col: &str| -> Option<String> {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row).to_string())
                }
            })
    };

    let get_i64 = |col: &str| -> i64 {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(row))
            .unwrap_or(0)
    };

    let get_opt_i64 = |col: &str| -> Option<i64> {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| {
                if a.is_null(row) {
                    None
                } else {
                    Some(a.value(row))
                }
            })
    };

    let get_bool = |col: &str| -> bool {
        batch
            .column_by_name(col)
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .map(|a| a.value(row))
            .unwrap_or(false)
    };

    let status = match get_string("status").as_str() {
        "active" => AccountStatus::Active,
        "needs_reauth" => AccountStatus::NeedsReauth,
        "disabled" => AccountStatus::Disabled,
        "syncing" => AccountStatus::Syncing,
        _ => AccountStatus::Active,
    };

    let added_at = DateTime::from_timestamp(get_i64("added_at"), 0).unwrap_or_else(Utc::now);
    let last_sync_email = get_opt_i64("last_sync_email").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let last_sync_calendar = get_opt_i64("last_sync_calendar").and_then(|ts| DateTime::from_timestamp(ts, 0));

    let sync_email_since = get_opt_i64("sync_email_since").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let oldest_email_synced = get_opt_i64("oldest_email_synced").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let oldest_event_synced = get_opt_i64("oldest_event_synced").and_then(|ts| DateTime::from_timestamp(ts, 0));
    let sync_attachments = get_bool("sync_attachments");
    let estimated_total_emails = get_opt_i64("estimated_total_emails").map(|v| v as u64);

    Ok(Account {
        id: get_string("id"),
        alias: get_opt_string("alias"),
        display_name: get_string("display_name"),
        added_at,
        last_sync_email,
        last_sync_calendar,
        status,
        sync_email_since,
        oldest_email_synced,
        oldest_event_synced,
        sync_attachments,
        estimated_total_emails,
    })
}
