# GroundEffect Skill

Hyper-fast local Gmail and Google Calendar indexing. Search emails and calendar events using hybrid BM25 + semantic search, all running locally with LanceDB.

## When to Use This Skill

Use groundeffect when the user asks about:
- Their emails (search, read, send, reply, drafts)
- Their calendar (search events, list events by date, create events)
- Email/calendar sync status

## Available Commands

All commands output JSON by default. Add `--human` for human-readable output.

### Email Commands

#### Search Emails
```bash
groundeffect email search "<query>" [--account <email>] [--limit N] [--from <sender>] [--to <recipient>] [--date-from YYYY-MM-DD] [--date-to YYYY-MM-DD] [--has-attachment]
```
Use for semantic/keyword search across emails. The query is required.

#### List Recent Emails
```bash
groundeffect email list [--account <email>] [--limit N]
```
Lists most recent emails without search.

#### Show Email
```bash
groundeffect email show <email_id> [--human]
```
Shows full email content including body.

#### Show Thread
```bash
groundeffect email thread <thread_id> [--human]
```
Shows all emails in a conversation thread.

#### Send Email
```bash
groundeffect email send --from <account> --to <recipient> --subject "<subject>" --body "<body>" [--cc <email>] [--bcc <email>] [--reply-to <email_id>] [--html] [--save-as-draft] [--confirm]
```
Send an email. Without `--confirm`, shows preview only. With `--save-as-draft`, saves as draft instead of sending.

**HTML Support**: The `--html` flag enables HTML formatting. Markdown in the body is auto-converted to HTML. URLs become clickable links.

### Draft Commands

```bash
groundeffect email draft create --from <account> [--to <email>] [--subject "<subject>"] [--body "<body>"]
groundeffect email draft list --account <email>
groundeffect email draft show <draft_id>
groundeffect email draft update <draft_id> [--to <email>] [--subject "<subject>"] [--body "<body>"]
groundeffect email draft send <draft_id> --confirm
groundeffect email draft delete <draft_id> --confirm
```

### Calendar Commands

#### Search Calendar Events (Semantic Search)
```bash
groundeffect calendar search "<query>" [--account <email>] [--limit N] [--after YYYY-MM-DD] [--before YYYY-MM-DD]
```
Use for semantic/keyword search across calendar events. **Requires a search query.**

#### List Calendar Events by Date Range (NO query required)
```bash
groundeffect calendar events [--from YYYY-MM-DD] [--to YYYY-MM-DD] [--account <email>] [--limit N] [--human]
```
**Use this to answer "what's on my calendar tomorrow" or "show me my meetings next week".**

This command lists ALL events in a date range chronologically. Unlike `calendar search`, it does NOT require a search query.

- `--from`: Start date (defaults to today)
- `--to`: End date (defaults to 7 days after start)
- `--account`: Filter to specific account(s), can be repeated
- `--human`: Shows events grouped by date with formatted times

**Examples:**
```bash
# Tomorrow's events
groundeffect calendar events --from 2026-01-07 --to 2026-01-08 --human

# Next 7 days (default)
groundeffect calendar events --human

# Specific account
groundeffect calendar events --from 2026-01-07 --account jamie@example.com
```

#### Show Calendar Event
```bash
groundeffect calendar show <event_id> [--human]
```
Shows full event details including description, attendees, location.

#### Create Calendar Event
```bash
groundeffect calendar create --account <email> --summary "<title>" --start "YYYY-MM-DDTHH:MM:SS" --end "YYYY-MM-DDTHH:MM:SS" [--description "<desc>"] [--location "<loc>"] [--attendees <email>] [--calendar <cal_id>]
```

### Account Commands

```bash
groundeffect account list                    # List all accounts
groundeffect account show <email>            # Show account details and sync window
groundeffect account add [--years N]         # Add new Google account
groundeffect account delete <email>          # Remove account
groundeffect account configure <email>       # Update settings
```

### Sync Commands

```bash
groundeffect sync status                                          # Show sync status
groundeffect sync extend --account <email> --target-date YYYY-MM-DD  # Sync older emails
groundeffect sync reset --account <email> --confirm               # Clear synced data
```

### Daemon Commands

```bash
groundeffect daemon status    # Check if running
groundeffect daemon restart   # Restart daemon
groundeffect daemon install   # Install launchd agent
groundeffect daemon uninstall # Remove launchd agent
```

## Common Patterns

### "What's on my calendar tomorrow?"
```bash
groundeffect calendar events --from 2026-01-07 --to 2026-01-08 --human
```

### "Search for emails about invoices from last month"
```bash
groundeffect email search "invoices" --date-from 2025-12-01 --date-to 2025-12-31
```

### "Show me my meetings for next week"
```bash
groundeffect calendar events --from 2026-01-06 --to 2026-01-13 --human
```

### "Find emails from John about the project"
```bash
groundeffect email search "project" --from john@example.com
```

### "Reply to this email"
```bash
groundeffect email send --from me@example.com --to recipient@example.com --subject "Re: Original Subject" --body "Reply content" --reply-to <original_email_id> --confirm
```

## MCP Tools (for AI integration)

The following MCP tools are available:

### Email Tools
| Tool | Description |
|------|-------------|
| `search_emails` | Hybrid BM25 + semantic search for emails |
| `list_emails` | List recent emails (faster than search) |
| `get_email` | Get full email content by ID |
| `get_thread` | Get all emails in a thread |
| `send_email` | Send or draft an email |
| `list_folders` | List IMAP folders |
| `get_attachment` | Get attachment content |

### Draft Tools
| Tool | Description |
|------|-------------|
| `create_draft` | Create a new draft |
| `list_drafts` | List drafts for an account |
| `get_draft` | Get draft content by ID |
| `update_draft` | Update an existing draft |
| `send_draft` | Send a draft |
| `delete_draft` | Delete a draft |

### Calendar Tools
| Tool | Description |
|------|-------------|
| `search_events` | Semantic search for calendar events |
| `list_events` | List events in a date range (no query needed) |
| `get_event` | Get full event details by ID |
| `list_calendars` | List all calendars |
| `create_event` | Create a new calendar event |

### Management Tools
| Tool | Description |
|------|-------------|
| `manage_accounts` | List/get/configure accounts |
| `manage_sync` | Get/control sync status |
| `manage_daemon` | Check daemon status |

### list_events MCP Tool

Use `list_events` when the user asks about their schedule without a specific search term:
- "What's on my calendar tomorrow?"
- "Show me my meetings next week"
- "Do I have anything scheduled for Friday?"

Parameters:
- `from`: Start date (YYYY-MM-DD), defaults to today
- `to`: End date (YYYY-MM-DD), defaults to 7 days from start
- `accounts`: Array of account emails to filter
- `limit`: Maximum results (default 50, max 200)

## Tips

1. **Use `calendar events` for date-based queries** - Don't use `calendar search` when the user just wants to see their schedule for a specific day/week.

2. **Use `--human` for readable output** - JSON is default, but `--human` is better for displaying to users.

3. **Check sync status if data seems missing** - Run `groundeffect sync status` to see sync windows.

4. **Multiple accounts** - Most commands support `--account` to filter to specific accounts.
