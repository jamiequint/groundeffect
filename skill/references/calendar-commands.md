# Calendar Commands Reference

## groundeffect calendar events

**List calendar events in a date range WITHOUT requiring a search query.**

Use this command when the user asks "what's on my calendar tomorrow" or "show me my meetings next week" - it simply lists all events chronologically in the specified date range.

```bash
groundeffect calendar events [options]
```

### Options
| Flag | Description | Default |
|------|-------------|---------|
| `--from` | Start date (YYYY-MM-DD) | today |
| `--to` | End date (YYYY-MM-DD) | 7 days after from |
| `--account` | Filter to specific account(s) | all accounts |
| `--limit` | Maximum results (1-200) | 50 |
| `--human` | Human-readable output grouped by date | JSON output |

### Examples
```bash
# Tomorrow's events (human-readable)
groundeffect calendar events --from 2026-01-07 --to 2026-01-08 --human

# Next 7 days (default range)
groundeffect calendar events --human

# Next 2 weeks for specific account
groundeffect calendar events --from 2026-01-06 --to 2026-01-20 --account jamie@example.com

# JSON output for processing
groundeffect calendar events --from 2026-01-07 --to 2026-01-08
```

### Output (Human-readable)
```
📅 Tuesday, Jan 7, 2026

   9:00 AM  Team Standup (30m)
            📍 Zoom

  12:00 PM  Lunch with Client (1h)
            📍 Restaurant Name

📅 Wednesday, Jan 8, 2026

   2:00 PM  Project Review (1h)
            📍 Conference Room A
```

### When to Use `calendar events` vs `calendar search`

| User Request | Command to Use |
|--------------|----------------|
| "What's on my calendar tomorrow?" | `calendar events --from ... --to ...` |
| "Show me my meetings next week" | `calendar events --from ... --to ...` |
| "Do I have anything Friday?" | `calendar events --from ... --to ...` |
| "Find meetings about the project" | `calendar search "project"` |
| "When did I last meet with John?" | `calendar search "John"` |

---

## groundeffect calendar search

Search calendar events using natural language.

```bash
groundeffect calendar search "query" [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--after` | Events after date (YYYY-MM-DD) | `--after 2024-01-01` |
| `--before` | Events before date (YYYY-MM-DD) | `--before 2024-12-31` |
| `--calendar` | Filter by calendar ID | `--calendar primary` |
| `--account` | Filter to specific account(s) | `--account work` |
| `--limit` | Number of results (1-100, default 10) | `--limit 25` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# Search for meetings this week
groundeffect calendar search "team meeting" --after 2024-01-08 --before 2024-01-15

# Find all events with a specific person
groundeffect calendar search "with John" --limit 20

# Search specific calendar
groundeffect calendar search "standup" --calendar team-calendar --account work
```

---

## groundeffect calendar list

List all available calendars.

```bash
groundeffect calendar list [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Filter to specific account(s) | `--account personal` |
| `--human` | Human-readable output | `--human` |

### Output Fields
- `id` - Calendar unique identifier
- `name` - Calendar display name
- `account` - Associated account
- `primary` - Whether this is the primary calendar

### Examples
```bash
# List all calendars across all accounts
groundeffect calendar list

# List calendars for specific account
groundeffect calendar list --account work --human
```

---

## groundeffect calendar show

Fetch a single calendar event by ID.

```bash
groundeffect calendar show <event_id> [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Output Fields
- `id` - Event unique identifier
- `summary` - Event title
- `start` - Start date/time (ISO 8601)
- `end` - End date/time (ISO 8601)
- `location` - Event location
- `description` - Event description
- `attendees` - List of attendees with response status
- `calendar_id` - Calendar this event belongs to
- `account` - Associated account

### Examples
```bash
# Show event details
groundeffect calendar show event_abc123

# Show with human-readable format
groundeffect calendar show event_abc123 --human
```

---

## groundeffect calendar create

Create a new calendar event.

```bash
groundeffect calendar create [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--summary` | Event title | Yes |
| `--start` | Start time (ISO 8601) | Yes |
| `--end` | End time (ISO 8601) | Yes |
| `--account` | Account to create event on | No (uses default) |
| `--calendar` | Calendar ID (omit for primary) | No |
| `--description` | Event description | No |
| `--location` | Event location | No |
| `--attendees` | Attendee emails (comma-separated) | No |
| `--human` | Human-readable output | No |

### Date/Time Format
Use ISO 8601 format for start and end times:
- With timezone: `2024-01-15T14:00:00-08:00`
- UTC: `2024-01-15T22:00:00Z`
- Local (assumes system timezone): `2024-01-15T14:00:00`

### Examples
```bash
# Create simple event
groundeffect calendar create \
  --summary "Team Standup" \
  --start "2024-01-15T09:00:00" \
  --end "2024-01-15T09:30:00"

# Create event with attendees and location
groundeffect calendar create \
  --summary "Project Review" \
  --start "2024-01-15T14:00:00-08:00" \
  --end "2024-01-15T15:00:00-08:00" \
  --location "Conference Room A" \
  --attendees "alice@example.com,bob@example.com" \
  --description "Q1 project progress review"

# Create on specific calendar
groundeffect calendar create \
  --summary "Personal Appointment" \
  --start "2024-01-16T10:00:00" \
  --end "2024-01-16T11:00:00" \
  --account personal \
  --calendar secondary-calendar
```
