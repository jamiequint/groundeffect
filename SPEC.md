# GroundEffect Specification

> A hyper-fast IMAP/CalDAV client and local MCP server for macOS

## Overview

GroundEffect is a high-performance, Mac-native email and calendar sync client that provides local storage, intelligent search, and programmatic access via the Model Context Protocol (MCP). It is designed for power users who want fast, offline-capable access to their Gmail and Google Calendar data through tools like Claude Code.

### Multi-Account Support

**GroundEffect MUST support multiple Gmail/Google Calendar accounts simultaneously.** This is a core requirement, not a future feature.

- Users can connect unlimited Gmail/GCal accounts
- Each account syncs independently with its own IMAP IDLE connection
- **Search is flexible**: query any single account, a subset of accounts, or all accounts at once
- All MCP tools support an `accounts` parameter for filtering
- Default behavior (no `accounts` specified) searches ALL connected accounts
- Account aliases (e.g., "work", "personal") can be configured for convenience

## Architecture

### Technology Stack

```
┌─────────────────────────────────────────────────────────────┐
│                    Swift/SwiftUI Shell                       │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ Menu Bar UI │  │ Status      │  │ ASWebAuth           │  │
│  │             │  │ Window      │  │ (OAuth Flow)        │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│                           │ UniFFI                           │
├───────────────────────────┼─────────────────────────────────┤
│                    Rust Core Library                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ Sync Engine │  │ MCP Server  │  │ Search Engine       │  │
│  │ (IMAP/      │  │ (stdio)     │  │ (uses LanceDB)      │  │
│  │  CalDAV)    │  │             │  │                     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│                           │                                  │
│  ┌──────────────────────────────────────────────────────┐   │
│  │                    LanceDB                            │   │
│  │  Storage + BM25 Full-Text Search + Vector ANN Search │   │
│  │  (ALL search functionality is built into LanceDB)    │   │
│  └──────────────────────────────────────────────────────┘   │
│                           │                                  │
│  ┌──────────────────────────────────────────────────────┐   │
│  │              Candle + Metal Acceleration              │   │
│  │  (nomic-embed-text-v1.5 / all-MiniLM-L6-v2)          │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Component Breakdown

> **Design Note**: This is a **macOS-only** application. Cross-platform portability is not a goal. The architecture prioritizes simplicity over abstraction layers.

| Component | Binary Name | Purpose |
|-----------|-------------|---------|
| **Daemon** | `groundeffect-daemon` | Long-running launchd service. Handles sync, indexing, writes to LanceDB. |
| **MCP Server** | `groundeffect-mcp` | CLI spawned by Claude. Opens LanceDB **read-only**. Handles search/retrieval. Mutations go to IMAP/CalDAV directly. |
| **Swift Shell** | `GroundEffect.app` | macOS UI. Menu bar, status window, OAuth flow, Keychain access. |

```
┌──────────────┐      ┌──────────────────┐
│ Claude Code  │─────►│ groundeffect-mcp │──read──► LanceDB
│ (MCP Host)   │stdio │ (read-only DB)   │            ▲
└──────────────┘      │                  │            │ write
                      │  mutations ──────┼──► IMAP/CalDAV
                      └──────────────────┘            │
                                                      │ sync
                      ┌──────────────────┐            │
                      │ groundeffect-    │────────────┘
                      │ daemon (writer)  │
                      └──────────────────┘
```

**Why this works without sockets:**
- **MCP is read-only for LanceDB**: Search, get_email, list_* only read data
- **Mutations bypass LanceDB**: send_email → IMAP, create_event → CalDAV. Daemon syncs changes back.
- **LanceDB supports concurrent readers**: Single writer (daemon) + multiple readers (MCP) = no lock contention
- **No Unix socket needed**: Simpler architecture, fewer moving parts

### Key Rust Crates

- `lancedb` - Embedded vector database with **built-in BM25 full-text search AND vector ANN search** (do NOT use separate search libraries)
- `candle-core`, `candle-nn`, `candle-transformers` - Local embedding model inference (generates vectors for LanceDB)
- `async-imap` / `imap` - IMAP client
- `icalendar` + HTTP client - CalDAV operations
- `rmcp` or custom - MCP server implementation (stdio JSON-RPC)
- `tokio` - Async runtime
- `governor` - Token bucket rate limiter (for Google API limits)
- `security-framework` - macOS Keychain access from Rust

> **Note**: LanceDB is the ONLY storage and search layer. It handles: data persistence, BM25 keyword search, vector similarity search, and filtering. No other database or search engine is needed.

> **Why Rust?** Not for portability—this is macOS-only. Rust is used because LanceDB and Candle are Rust-native libraries with no Swift equivalents. The Swift shell handles UI only.

---

## Email Sync

### Provider Support

- **Gmail only** (via OAuth 2.0)
- Uses Gmail IMAP with XOAUTH2 authentication
- Leverages Gmail-specific IMAP extensions (X-GM-THRID, X-GM-MSGID, X-GM-LABELS)

### Sync Strategy

| Aspect | Behavior |
|--------|----------|
| **Initial Sync (Phase 1)** | Smart window: Last 90 days + all unread/flagged. App is usable immediately. |
| **Initial Sync (Phase 2)** | Background backfill: Older emails fetched in reverse chronological order, low priority. |
| **Attachments** | **Lazy download**: Index metadata (filename, size, MIME type) immediately. Download content on-demand via MCP resource URI or background task. |
| **Incremental Sync** | IMAP IDLE for real-time push notifications |
| **Fallback** | Poll every 5 minutes if IDLE disconnects |
| **Concurrency** | Parallel folder sync, batched message fetches (rate-limited, see below) |
| **Multi-Account** | Each account has its own IMAP connection and IDLE listener |
| **Isolation** | Account sync failures don't affect other accounts |

> **Note on "Full Sync"**: The goal is eventual full sync of all history. But initial sync prioritizes recent/important emails so the app is usable within minutes, not hours. Backfill runs in the background.

### Data Stored Per Email

```rust
struct Email {
    // Identifiers
    id: String,                    // Internal UUID
    account_id: String,            // Account identifier (email address)
    account_alias: Option<String>, // User-defined alias (e.g., "work", "personal")
    message_id: String,            // RFC 5322 Message-ID
    gmail_message_id: u64,         // X-GM-MSGID
    gmail_thread_id: u64,          // X-GM-THRID
    uid: u32,                      // IMAP UID

    // Threading (standards-based fallback)
    in_reply_to: Option<String>,
    references: Vec<String>,

    // Metadata
    folder: String,
    labels: Vec<String>,           // Gmail labels
    flags: Vec<String>,            // IMAP flags (Seen, Flagged, etc.)

    // Headers
    from: Address,
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    subject: String,
    date: DateTime<Utc>,

    // Content
    body_plain: String,
    body_html: Option<String>,
    snippet: String,               // First ~200 chars for preview

    // Attachments
    attachments: Vec<Attachment>,

    // Search
    embedding: Vector<f32, 768>,   // Fixed 768 dimensions (nomic-embed-text-v1.5)

    // Sync metadata
    synced_at: DateTime<Utc>,
    raw_size: u64,
}

struct Attachment {
    id: String,
    filename: String,
    mime_type: String,
    size: u64,
    local_path: PathBuf,          // Local file path
    content_id: Option<String>,   // For inline attachments
}
```

### Threading

1. **Primary**: Use Gmail Thread ID (`X-GM-THRID`) for instant, zero-compute threading
2. **Secondary**: Store `Message-ID`, `In-Reply-To`, and `References` headers for:
   - Future non-Gmail provider support
   - Handling edge cases (subject line changes, split threads)
   - Data portability

---

## Calendar Sync

### Provider Support

- **Google Calendar only** (via OAuth 2.0)
- CalDAV protocol with Google's CalDAV endpoint
- Same OAuth token as Gmail (single auth flow)

### Sync Strategy

| Aspect | Behavior |
|--------|----------|
| **Scope** | Full history (all past and future events) |
| **Poll Interval** | Every 1-5 minutes |
| **Sync Token** | Use CalDAV sync-token for efficient incremental sync |

### Data Stored Per Event

```rust
struct CalendarEvent {
    // Identifiers
    id: String,                    // Internal UUID
    account_id: String,            // Account identifier (email address)
    account_alias: Option<String>, // User-defined alias (e.g., "work", "personal")
    google_event_id: String,       // Google Calendar event ID
    ical_uid: String,              // iCalendar UID
    etag: String,                  // For change detection

    // Event data
    summary: String,
    description: Option<String>,
    location: Option<String>,

    // Timing
    start: EventTime,
    end: EventTime,
    timezone: String,
    all_day: bool,

    // Recurrence
    recurrence_rule: Option<String>,  // RRULE
    recurrence_id: Option<String>,    // For exceptions

    // Attendees
    organizer: Option<Attendee>,
    attendees: Vec<Attendee>,

    // Status
    status: EventStatus,           // Confirmed, Tentative, Cancelled
    transparency: Transparency,    // Opaque (busy) or Transparent (free)

    // Reminders
    reminders: Vec<Reminder>,

    // Search
    embedding: Vector<f32, 768>,   // Fixed 768 dimensions (nomic-embed-text-v1.5)

    // Sync metadata
    calendar_id: String,
    synced_at: DateTime<Utc>,
}

enum EventTime {
    DateTime(DateTime<Utc>),
    Date(NaiveDate),
}
```

---

## Search Engine

### Architecture

> **IMPORTANT**: Both BM25 full-text search and vector search are **built-in LanceDB features**. Do NOT implement custom BM25 or vector search algorithms. LanceDB provides both out of the box via its FTS (Full-Text Search) and ANN (Approximate Nearest Neighbor) capabilities.

GroundEffect uses a hybrid search approach combining BM25 (keyword) and vector (semantic) search, **both provided natively by LanceDB**:

```
Query → ┬→ BM25 Search (LanceDB FTS) ──────→ Score normalization ─┬→ RRF Fusion → Results
        └→ Vector Search (LanceDB ANN) ──→ Score normalization ──┘
```

### LanceDB Search Capabilities

LanceDB provides everything needed for hybrid search:

| Feature | LanceDB Capability | Usage |
|---------|-------------------|-------|
| **Full-Text Search (BM25)** | Built-in FTS with `create_fts_index()` | Keyword matching, relevance scoring |
| **Vector Search (ANN)** | Native vector columns + `search().nearest()` | Semantic similarity via embeddings |
| **Hybrid Search** | Combine FTS + vector in single query | Use LanceDB's reranking or custom RRF |
| **Filtering** | SQL-like `where()` clauses | Filter by account_id, date, folder, etc. |

**Do NOT:**
- Implement custom BM25 scoring
- Build custom inverted indexes
- Implement custom ANN/HNSW algorithms
- Use external search libraries (tantivy, meilisearch, etc.)

**DO:**
- Use `lancedb` crate's built-in FTS and vector search
- Configure LanceDB indexes appropriately
- Implement RRF fusion to combine LanceDB's BM25 and vector scores

### Multi-Account Search

Search queries can target any combination of accounts:

| `accounts` Parameter | Behavior |
|---------------------|----------|
| `null` / omitted | Search ALL connected accounts |
| `["work@gmail.com"]` | Search single account |
| `["work@gmail.com", "personal@gmail.com"]` | Search specific accounts |
| `["work"]` | Search by alias (if configured) |
| `["*"]` | Explicit "all accounts" |

Results from multi-account searches are:
1. Merged into a single result set
2. Scored uniformly (BM25 + vector scores are comparable across accounts)
3. Sorted by relevance (RRF score)
4. Each result includes `account_id` and `account_alias` for identification

### Embedding Model

| Setting | Value |
|---------|-------|
| **Framework** | Hugging Face Candle (Rust-native) |
| **Model** | `nomic-embed-text-v1.5` (quantized 4-bit GGUF) |
| **Alternative** | `all-MiniLM-L6-v2` (faster, smaller) |
| **Dimensions** | **768 (fixed)** — see note below |
| **Acceleration** | Metal (macOS GPU/Neural Engine) |

> **IMPORTANT: Fixed Vector Dimensions**. LanceDB requires a fixed vector dimension in the schema. You CANNOT have different dimension vectors in the same table. The dimension (768) is chosen at schema creation time and cannot be changed without a full re-index. While `nomic-embed-text-v1.5` supports Matryoshka truncation (256, 384, 512, 768), we fix at 768 for v1 to maximize search quality. If you need to change dimensions later, you must re-embed all documents.

### Performance Targets

| Metric | Target |
|--------|--------|
| **Search latency** | < 100ms |
| **Embedding latency** | < 50ms per email |
| **Index size** | ~1KB per email (embedding + metadata) |

### Search Fields

**Email:**
- Subject (weighted higher)
- Body (plain text)
- Sender name and address
- Attachment filenames

**Calendar:**
- Summary (title)
- Description
- Location
- Attendee names

### Reciprocal Rank Fusion (RRF)

Results from BM25 and vector search are combined using RRF:

```
RRF_score(d) = Σ 1 / (k + rank_i(d))
```

Where `k = 60` (standard constant) and `rank_i(d)` is the rank of document `d` in result set `i`.

---

## MCP Server

### Transport

- **stdio** (standard input/output)
- Binary: `groundeffect-mcp`
- Spawned by Claude Code as subprocess
- JSON-RPC 2.0 protocol per MCP specification

### Read/Write Separation

The MCP server does NOT write to LanceDB. This avoids lock contention with the daemon:

| Operation | LanceDB | Remote API |
|-----------|---------|------------|
| `search_emails`, `search_calendar` | READ | - |
| `get_email`, `get_event`, `get_thread` | READ | - |
| `list_folders`, `list_calendars`, `list_accounts` | READ | - |
| `get_sync_status` | READ | - |
| `send_email` | - | WRITE to IMAP |
| `create_event`, `update_event`, `delete_event` | - | WRITE to CalDAV |
| `delete_email`, `move_email`, `archive_email` | - | WRITE to IMAP |
| `mark_read`, `mark_unread` | - | WRITE to IMAP |

After mutations, the daemon syncs changes from remote back to LanceDB on its normal sync cycle (or immediately via IMAP IDLE notification).

### Tools

> **Multi-Account Parameter**: Most tools accept an `accounts` parameter (array of email addresses or aliases). When omitted, the tool operates on ALL accounts. When specified, it filters to only those accounts.

#### Account Tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `list_accounts` | List all connected accounts | - |
| `get_account` | Get details for a specific account | `account` (email or alias) |

#### Email Tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `search_emails` | Hybrid BM25 + vector search | `query`, `accounts?`, `limit?`, `folder?`, `from?`, `to?`, `date_from?`, `date_to?`, `has_attachment?` |
| `get_email` | Fetch single email by ID | `id` |
| `get_thread` | Fetch all emails in a thread | `thread_id`, `accounts?` |
| `list_folders` | List all IMAP folders | `accounts?` |
| `send_email` | Compose and send email | `from_account`, `to`, `subject`, `body`, `cc?`, `bcc?`, `attachments?`, `reply_to_message_id?` |
| `delete_email` | Move email to trash | `id` |
| `move_email` | Move email to folder | `id`, `folder` |
| `archive_email` | Archive email (remove from Inbox) | `id` |
| `mark_read` | Mark email as read | `id` |
| `mark_unread` | Mark email as unread | `id` |

#### Calendar Tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `search_calendar` | Search events | `query`, `accounts?`, `limit?`, `calendar_id?`, `date_from?`, `date_to?` |
| `get_event` | Fetch single event by ID | `id` |
| `list_calendars` | List all calendars | `accounts?` |
| `create_event` | Create new event | `account`, `summary`, `start`, `end`, `calendar_id?`, `description?`, `location?`, `attendees?`, `reminders?` |
| `update_event` | Update existing event | `id`, `summary?`, `start?`, `end?`, `description?`, `location?` |
| `delete_event` | Delete event | `id` |

#### System Tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `get_sync_status` | Get sync status and stats | `accounts?` |
| `trigger_sync` | Force immediate sync | `accounts?`, `type?` (email, calendar, all) |

### Resources

MCP resource URIs for direct content access:

| URI Pattern | Description |
|-------------|-------------|
| `email://{message_id}` | Raw email content |
| `email://{message_id}/body` | Email body (plain text) |
| `email://{message_id}/attachments/{filename}` | Attachment content |
| `calendar://{event_id}` | Event details (iCalendar format) |

### Response Format

All tool responses return hybrid JSON + Markdown:

```json
{
  "results": [
    {
      "id": "abc123",
      "account_id": "work@gmail.com",
      "account_alias": "work",
      "message_id": "<uuid@mail.gmail.com>",
      "thread_id": "thread_xyz",
      "from": {"name": "John Doe", "email": "john@example.com"},
      "to": [{"name": "Jane Doe", "email": "jane@example.com"}],
      "subject": "Project Update",
      "date": "2024-01-15T10:30:00Z",
      "snippet": "Here's the latest update on...",
      "has_attachments": true,
      "labels": ["INBOX", "IMPORTANT"],
      "markdown_summary": "**Account:** work@gmail.com (work)\n**From:** John Doe <john@example.com>\n**Subject:** Project Update\n**Date:** Jan 15, 2024 10:30 AM\n\nHere's the latest update on..."
    }
  ],
  "accounts_searched": ["work@gmail.com", "personal@gmail.com"],
  "total_count": 42,
  "search_time_ms": 23
}
```

---

## Authentication

### Multi-Account OAuth

GroundEffect supports **unlimited Gmail/GCal accounts**. Each account:
- Has its own OAuth tokens stored separately in Keychain
- Syncs independently with dedicated IMAP IDLE connection
- Can be assigned a user-friendly alias

### OAuth 2.0 Flow

1. User clicks "Add Account" in menu bar
2. `ASWebAuthenticationSession` opens native auth sheet
3. User authenticates with Google (or selects existing Google session)
4. Callback received with authorization code
5. Exchange code for access + refresh tokens
6. Store tokens in macOS Keychain (keyed by email address)
7. Optionally prompt user for account alias (e.g., "work", "personal")
8. Begin initial sync for the new account

### Scopes Required

```
https://mail.google.com/                     # Full Gmail access (IMAP)
https://www.googleapis.com/auth/gmail.send   # Send emails
https://www.googleapis.com/auth/calendar     # Full Calendar access
https://www.googleapis.com/auth/userinfo.email  # Get email address for account ID
```

### Token Storage

Tokens are stored per-account in macOS Keychain:

| Item | Storage Location |
|------|------------------|
| Access Token | Keychain: `com.groundeffect.oauth.{email}` |
| Refresh Token | Keychain: `com.groundeffect.oauth.{email}` |
| Token Expiry | Keychain (as metadata) |
| Account Alias | Config file (not sensitive) |

### Token Refresh

- Automatic refresh when access token expires (typically 1 hour)
- Background refresh 5 minutes before expiry
- Per-account refresh (one failing doesn't affect others)
- If refresh fails, mark account as "needs re-auth" and continue syncing other accounts

### Account Management

```rust
struct Account {
    id: String,                    // Email address (primary key)
    alias: Option<String>,         // User-defined alias
    display_name: String,          // From Google profile
    added_at: DateTime<Utc>,
    last_sync_email: Option<DateTime<Utc>>,
    last_sync_calendar: Option<DateTime<Utc>>,
    status: AccountStatus,         // Active, NeedsReauth, Disabled
}

enum AccountStatus {
    Active,
    NeedsReauth,
    Disabled,
    Syncing,
}
```

---

## User Interface

### Menu Bar

```
┌─────────────────────────────────┐
│ ⚡ GroundEffect          [icon] │
├─────────────────────────────────┤
│ Accounts:                       │
│   ✓ work@gmail.com (work)       │
│   ✓ personal@gmail.com          │
│   ⚠ old@gmail.com (needs auth)  │
│ ─────────────────────────────── │
│ 📧 4,567 emails (3 accounts)    │
│ 📅 890 events (3 accounts)      │
│ Last sync: 2 minutes ago        │
│ ─────────────────────────────── │
│ Recent:                         │
│   • [work] Email from John Doe  │
│   • [personal] Meeting: Dentist │
│   • [work] Email from Jane      │
│ ─────────────────────────────── │
│ ➕ Add Account...                │
│ ⟳ Sync All Now                  │
│ 📊 Open Status Window           │
│ ⚙ Preferences...                │
│ ─────────────────────────────── │
│ Quit GroundEffect               │
└─────────────────────────────────┘
```

### Status Window

Optional window showing:

- **Accounts List**: All connected accounts with status, sync times, item counts
- **Per-Account Sync Status**: Individual progress bars and status
- **Aggregate Statistics**: Total emails, events, index size across all accounts
- **Recent Activity**: Log of sync operations (tagged by account)
- **Search Test**: Input field to test search queries with account filter dropdown
- **Account Management**: Add, remove, rename alias, re-authenticate accounts

---

## Configuration

### File Location

```
~/.config/groundeffect/config.toml
```

### Schema

```toml
[general]
log_level = "info"                    # debug, info, warn, error
log_file = "~/.local/share/groundeffect/groundeffect.log"
data_dir = "~/.local/share/groundeffect/data"

[sync]
email_idle_enabled = true             # Use IMAP IDLE for real-time push
email_poll_interval_secs = 300        # Fallback poll interval
calendar_poll_interval_secs = 300     # CalDAV poll interval
max_concurrent_fetches = 10           # Parallel email fetches per account
attachment_max_size_mb = 100          # Skip attachments larger than this

[search]
embedding_model = "nomic-embed-text-v1.5"  # or "all-MiniLM-L6-v2"
# Note: embedding_dimensions is FIXED at 768 in the schema. Changing requires full re-index.
use_metal = true                      # Metal GPU acceleration
bm25_weight = 0.5                     # Weight for BM25 in hybrid search
vector_weight = 0.5                   # Weight for vector in hybrid search

[ui]
show_menu_bar_icon = true
show_recent_items = 5
launch_at_login = false

# Account aliases - map friendly names to email addresses
# These can be used in MCP tool `accounts` parameter
[accounts.aliases]
work = "jamie@company.com"
personal = "jamie.personal@gmail.com"
side-project = "jamie@startup.io"

# Per-account overrides (optional)
[accounts."jamie@company.com"]
sync_enabled = true
# Can override sync settings per account if needed

[accounts."jamie.personal@gmail.com"]
sync_enabled = true

[accounts."jamie@startup.io"]
sync_enabled = false                  # Temporarily disable syncing this account
```

---

## Data Storage

### Directory Structure

```
~/.local/share/groundeffect/
├── data/
│   ├── lancedb/                 # LanceDB database files (shared across accounts)
│   │   ├── emails.lance/        # All emails, partitioned by account_id
│   │   ├── events.lance/        # All events, partitioned by account_id
│   │   └── accounts.lance/      # Account metadata
│   ├── attachments/             # Downloaded attachments (organized by account)
│   │   └── {account_id}/
│   │       └── {message_id}/
│   │           └── {filename}
│   └── models/                  # Embedding model files (shared)
│       └── nomic-embed-text-v1.5.gguf
├── logs/
│   └── groundeffect.log
└── cache/
    └── sync_state/              # Per-account sync state
        ├── {account_id_1}.json  # IMAP UIDs, CalDAV sync tokens
        └── {account_id_2}.json
```

### Multi-Account Data Design

- **Single LanceDB instance** with `account_id` column for filtering
- **Partition by account** for efficient single-account queries
- **Global index** for cross-account search
- **Isolated sync state** per account (one failing doesn't corrupt others)
- **Fixed schema**: Vector dimensions (768) are set at table creation and cannot change without re-indexing all data

### Security

- **Encryption**: Rely on macOS FileVault for at-rest encryption
- **OAuth Tokens**: Stored in macOS Keychain (encrypted by OS)
- **Attachments**: Stored as plain files (protected by FileVault)
- **No app-level encryption**: Keeps implementation simple, trusts OS security

---

## Background Daemon

### Implementation

- **launchd** service for macOS
- Starts at login (optional, configurable)
- Runs continuously in background
- Minimal resource usage when idle

### Launch Agent

```xml
<!-- ~/Library/LaunchAgents/com.groundeffect.daemon.plist -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.groundeffect.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/GroundEffect.app/Contents/MacOS/groundeffect-daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/groundeffect.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/groundeffect.stderr.log</string>
</dict>
</plist>
```

### Process Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      macOS System                            │
│                                                              │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐  │
│  │ GroundEffect │    │ groundeffect │    │ Claude Code  │  │
│  │   Menu Bar   │◄──►│   -daemon    │◄──►│ (MCP Client) │  │
│  │   (Swift)    │    │   (Rust)     │    │              │  │
│  └──────────────┘    └──────────────┘    └──────────────┘  │
│         │                   │                    │          │
│         │ UniFFI            │ TCP/Unix Socket    │ stdio    │
│         ▼                   ▼                    ▼          │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Rust Core Library (libgroundeffect)      │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

---

## Email Composition

### send_email Tool

Supports:
- **Plain text only** (no HTML)
- **Attachments** from local file paths
- **CC/BCC** recipients
- **Reply threading** via `reply_to_message_id` parameter

```json
{
  "tool": "send_email",
  "params": {
    "to": ["recipient@example.com"],
    "cc": ["cc@example.com"],
    "bcc": ["bcc@example.com"],
    "subject": "Re: Project Update",
    "body": "Thanks for the update!\n\nBest regards",
    "attachments": ["/path/to/file.pdf"],
    "reply_to_message_id": "<original-message-id@mail.gmail.com>"
  }
}
```

### Threading Behavior

When `reply_to_message_id` is provided:
1. Fetch original email's `Message-ID`, `Subject`, `References`
2. Set `In-Reply-To` header to original `Message-ID`
3. Set `References` header to original's `References` + original `Message-ID`
4. Prefix subject with "Re: " if not already present
5. Gmail will automatically thread the reply

---

## Logging

### Format

```
2024-01-15T10:30:45.123Z INFO  [groundeffect::sync] Starting email sync for folder=INBOX
2024-01-15T10:30:45.456Z DEBUG [groundeffect::imap] IDLE notification received: EXISTS 1234
2024-01-15T10:30:45.789Z INFO  [groundeffect::sync] Fetched 5 new emails in 234ms
2024-01-15T10:30:46.012Z WARN  [groundeffect::oauth] Token expires in 5 minutes, refreshing
```

### Log Levels

| Level | Description |
|-------|-------------|
| `error` | Failures requiring attention |
| `warn` | Potential issues, degraded operation |
| `info` | Normal operation events |
| `debug` | Detailed debugging information |

### Log Rotation

- Rotate when file exceeds 10MB
- Keep last 5 rotated files
- Configurable via `config.toml`

---

## Error Handling

### Retry Strategy

| Error Type | Strategy |
|------------|----------|
| Network timeout | Exponential backoff (1s, 2s, 4s, 8s, max 60s) |
| OAuth token expired | Automatic refresh, then retry |
| IMAP connection lost | Reconnect with backoff, resume IDLE |
| Rate limiting | Respect `Retry-After` header |

### Error Reporting

- Errors logged to file
- Critical errors shown in menu bar (⚠️ icon)
- MCP tools return structured error responses

```json
{
  "error": {
    "code": "AUTH_EXPIRED",
    "message": "OAuth token expired and refresh failed",
    "action": "Please re-authenticate in GroundEffect preferences"
  }
}
```

---

## Performance Considerations

### Rate Limiting

> **CRITICAL**: Google aggressively rate-limits and bans apps that make too many concurrent requests. Implement a global rate limiter.

| Limit | Value | Reason |
|-------|-------|--------|
| **Requests/second** | 10 max (global across all accounts) | Avoid Google throttling |
| **Concurrent IMAP connections** | 1 per account | Gmail limit |
| **Concurrent large body fetches** | 3 max total | Bandwidth management |
| **Attachment download rate** | 1 at a time | Avoid bandwidth spikes |
| **Backfill fetch rate** | 100 emails/minute | Stay under radar during initial sync |

**Implementation**: Use a `TokenBucket` rate limiter (e.g., `governor` crate) shared across all sync operations.

### Memory Management

- Stream large emails instead of loading entirely into memory
- Limit concurrent embedding operations (max 4)
- Use memory-mapped files for LanceDB access

### Disk I/O

- Batch writes to LanceDB (commit every 100 items or 5 seconds)
- Async attachment downloads
- Background indexing (don't block sync)

### Network

- Connection pooling for IMAP
- HTTP/2 for CalDAV requests
- Compress sync state for efficient storage

---

## Future Considerations

*Not in scope for v1, but could be added later:*

- Additional email providers (iCloud, Fastmail, generic IMAP)
- JMAP protocol support
- MCP HTTP/SSE transport (for non-Claude clients)
- Full-text search within attachments (PDF, DOCX)
- Local LLM integration for summarization
- Shared family/team workspaces

> **Note**: Cross-platform support (Linux, Windows) is explicitly NOT a goal. This is a macOS-only application.

---

## Development Phases

### Phase 1: Foundation
- [ ] Rust workspace with two binary targets: `groundeffect-daemon` and `groundeffect-mcp`
- [ ] LanceDB schema with account_id partitioning and fixed 768-dim vectors
- [ ] Account management data structures
- [ ] Candle embedding pipeline with Metal
- [ ] Keychain integration for OAuth tokens (`security-framework` crate)
- [ ] OAuth flow (hardcoded client ID for dev)

### Phase 2: Multi-Account Infrastructure
- [ ] Account registry and management
- [ ] Per-account Keychain token storage
- [ ] Account alias configuration
- [ ] Concurrent sync orchestration
- [ ] Global rate limiter (TokenBucket via `governor` crate)

### Phase 3: Email Sync
- [ ] Gmail IMAP connection with XOAUTH2
- [ ] Smart initial sync: Last 90 days + unread/flagged first
- [ ] Background backfill for older emails (rate-limited)
- [ ] IMAP IDLE implementation (per-account connections)
- [ ] Lazy attachment handling: index metadata, download on-demand

### Phase 4: Calendar Sync
- [ ] CalDAV client implementation
- [ ] Event parsing and storage (with account_id)
- [ ] Incremental sync with sync-token

### Phase 5: Search
- [ ] Configure LanceDB FTS index for BM25 search (account-aware)
- [ ] Configure LanceDB vector index for embedding search
- [ ] Implement RRF fusion to combine LanceDB's BM25 + vector scores
- [ ] Multi-account search with LanceDB `where()` filtering
- [ ] Search performance optimization
- [ ] **Note: Use LanceDB's built-in search - do NOT implement custom BM25/vector search**

### Phase 6: MCP Server
- [ ] stdio JSON-RPC implementation
- [ ] Read-only LanceDB access for search/retrieval
- [ ] Direct IMAP/CalDAV writes for mutations
- [ ] Account tools (list_accounts, get_account)
- [ ] Tool handlers with `accounts` parameter support
- [ ] Resource URI handlers
- [ ] Integration testing with Claude Code

### Phase 7: macOS Integration
- [ ] Swift app shell (menu bar + status window)
- [ ] Menu bar UI (multi-account display)
- [ ] Status window (per-account status, sync logs)
- [ ] Add Account flow with ASWebAuthenticationSession
- [ ] launchd plist for daemon auto-start
- [ ] App bundle with embedded Rust binaries

### Phase 8: Polish
- [ ] Error handling improvements (per-account isolation)
- [ ] Performance profiling and optimization
- [ ] Logging and observability
- [ ] Documentation

---

## Appendix: MCP Tool Schemas

### search_emails

```json
{
  "name": "search_emails",
  "description": "Search emails using hybrid BM25 + vector search across one or more accounts",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {"type": "string", "description": "Search query (natural language)"},
      "accounts": {
        "type": "array",
        "items": {"type": "string"},
        "description": "Account(s) to search (email addresses or aliases). Omit to search ALL accounts."
      },
      "limit": {"type": "integer", "default": 10, "maximum": 100},
      "folder": {"type": "string", "description": "Filter by folder (e.g., INBOX, Sent)"},
      "from": {"type": "string", "description": "Filter by sender email/name"},
      "to": {"type": "string", "description": "Filter by recipient email/name"},
      "date_from": {"type": "string", "format": "date", "description": "Filter emails after this date"},
      "date_to": {"type": "string", "format": "date", "description": "Filter emails before this date"},
      "has_attachment": {"type": "boolean", "description": "Filter emails with attachments"}
    },
    "required": ["query"]
  }
}
```

**Example: Search all accounts**
```json
{"query": "quarterly report"}
```

**Example: Search work account only**
```json
{"query": "quarterly report", "accounts": ["work"]}
```

**Example: Search multiple specific accounts**
```json
{"query": "quarterly report", "accounts": ["work@gmail.com", "personal@gmail.com"]}
```

### send_email

```json
{
  "name": "send_email",
  "description": "Compose and send a plain text email from a specific account",
  "inputSchema": {
    "type": "object",
    "properties": {
      "from_account": {"type": "string", "description": "Account to send from (email or alias). REQUIRED."},
      "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient email addresses"},
      "subject": {"type": "string", "description": "Email subject"},
      "body": {"type": "string", "description": "Plain text email body"},
      "cc": {"type": "array", "items": {"type": "string"}, "description": "CC recipients"},
      "bcc": {"type": "array", "items": {"type": "string"}, "description": "BCC recipients"},
      "attachments": {"type": "array", "items": {"type": "string"}, "description": "Local file paths to attach"},
      "reply_to_message_id": {"type": "string", "description": "Message-ID to reply to (for threading)"}
    },
    "required": ["from_account", "to", "subject", "body"]
  }
}
```

### create_event

```json
{
  "name": "create_event",
  "description": "Create a new calendar event on a specific account",
  "inputSchema": {
    "type": "object",
    "properties": {
      "account": {"type": "string", "description": "Account to create event on (email or alias). REQUIRED."},
      "calendar_id": {"type": "string", "description": "Calendar ID within the account (omit for primary calendar)"},
      "summary": {"type": "string", "description": "Event title"},
      "start": {"type": "string", "format": "date-time", "description": "Start time (ISO 8601)"},
      "end": {"type": "string", "format": "date-time", "description": "End time (ISO 8601)"},
      "description": {"type": "string", "description": "Event description"},
      "location": {"type": "string", "description": "Event location"},
      "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee email addresses"},
      "reminders": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "method": {"type": "string", "enum": ["popup", "email"]},
            "minutes": {"type": "integer"}
          }
        }
      }
    },
    "required": ["account", "summary", "start", "end"]
  }
}
```

### search_calendar

```json
{
  "name": "search_calendar",
  "description": "Search calendar events across one or more accounts",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {"type": "string", "description": "Search query (natural language)"},
      "accounts": {
        "type": "array",
        "items": {"type": "string"},
        "description": "Account(s) to search (email addresses or aliases). Omit to search ALL accounts."
      },
      "limit": {"type": "integer", "default": 10, "maximum": 100},
      "calendar_id": {"type": "string", "description": "Filter to specific calendar within account(s)"},
      "date_from": {"type": "string", "format": "date", "description": "Filter events after this date"},
      "date_to": {"type": "string", "format": "date", "description": "Filter events before this date"}
    },
    "required": ["query"]
  }
}
```

### get_sync_status

```json
{
  "name": "get_sync_status",
  "description": "Get current sync status and statistics for all or specified accounts",
  "inputSchema": {
    "type": "object",
    "properties": {
      "accounts": {
        "type": "array",
        "items": {"type": "string"},
        "description": "Filter to specific accounts (email or alias). Omit for all accounts."
      }
    }
  }
}
```

**Response:**

```json
{
  "accounts": [
    {
      "id": "work@gmail.com",
      "alias": "work",
      "status": "idle",
      "last_email_sync": "2024-01-15T10:30:00Z",
      "last_calendar_sync": "2024-01-15T10:28:00Z",
      "email_count": 8234,
      "event_count": 345,
      "attachment_count": 567
    },
    {
      "id": "personal@gmail.com",
      "alias": "personal",
      "status": "syncing",
      "last_email_sync": "2024-01-15T10:25:00Z",
      "last_calendar_sync": "2024-01-15T10:28:00Z",
      "email_count": 4111,
      "event_count": 222,
      "attachment_count": 323
    }
  ],
  "totals": {
    "email_count": 12345,
    "event_count": 567,
    "attachment_count": 890,
    "index_size_mb": 234.5,
    "attachment_storage_mb": 1024.0
  }
}
```

### list_accounts

```json
{
  "name": "list_accounts",
  "description": "List all connected Gmail/GCal accounts",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

**Response:**

```json
{
  "accounts": [
    {
      "id": "work@gmail.com",
      "alias": "work",
      "display_name": "Jamie Quinn",
      "status": "active",
      "added_at": "2024-01-01T00:00:00Z"
    },
    {
      "id": "personal@gmail.com",
      "alias": null,
      "display_name": "Jamie Q",
      "status": "active",
      "added_at": "2024-01-05T00:00:00Z"
    },
    {
      "id": "old@gmail.com",
      "alias": null,
      "display_name": "Jamie Old",
      "status": "needs_reauth",
      "added_at": "2023-06-01T00:00:00Z"
    }
  ]
}
```
