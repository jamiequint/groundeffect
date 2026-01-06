# Common Workflows

## Initial Setup

### Adding Your First Account

```bash
# Add a Google account with 3 years of history
ge account add --alias work --years 3

# Browser opens for OAuth - authorize GroundEffect
# Once authorized, sync begins automatically

# Check sync progress
ge sync status --account work --human

# Start daemon for continuous sync
ge daemon start --logging
```

### Adding Multiple Accounts

```bash
# Add work account
ge account add --alias work --years 5

# Add personal account
ge account add --alias personal --years 2

# List all accounts
ge account list --human
```

---

## Email Workflows

### Finding Recent Emails from a Person

```bash
# Search for emails from a specific sender
ge email search "from:john@company.com" --limit 20 --human

# Or use the --from flag
ge email search "" --from "john@company.com" --limit 20 --human
```

### Searching for Emails in a Date Range

```bash
# Find project-related emails from last month
ge email search "project update" --after 2024-12-01 --before 2024-12-31

# Find emails with attachments from Q4
ge email search "report" --after 2024-10-01 --has-attachment --limit 50
```

### Reading an Email Thread

```bash
# First, search for the email
ge email search "contract negotiation" --limit 5

# Get the thread_id from the result, then fetch full thread
ge email thread 18abc123def456 --human
```

### Downloading an Attachment

```bash
# Show email to see attachments
ge email show email_abc123 --human

# Download specific attachment
ge email attachment email_abc123 "contract.pdf"
```

### Sending an Email

```bash
# Preview the email first (default behavior)
ge email send \
  --to "recipient@example.com" \
  --subject "Project Update" \
  --body "Hi, here is the latest project status..."

# Review the preview, then send with confirmation
ge email send \
  --to "recipient@example.com" \
  --subject "Project Update" \
  --body "Hi, here is the latest project status..." \
  --confirm
```

### Replying to an Email

```bash
# Get the email ID from search or list
ge email show original_email_id

# Send reply (uses --reply-to for threading)
ge email send \
  --to "sender@example.com" \
  --subject "Re: Original Subject" \
  --body "Thanks for your message..." \
  --reply-to original_email_id \
  --confirm
```

---

## Calendar Workflows

### Finding Upcoming Meetings

```bash
# Search for meetings in the next week
ge calendar search "meeting" --after 2024-01-15 --before 2024-01-22 --human
```

### Creating a Meeting

```bash
# Create a simple meeting
ge calendar create \
  --summary "Team Standup" \
  --start "2024-01-16T09:00:00" \
  --end "2024-01-16T09:30:00"

# Create meeting with attendees and location
ge calendar create \
  --summary "Project Review" \
  --start "2024-01-16T14:00:00-08:00" \
  --end "2024-01-16T15:00:00-08:00" \
  --location "Conference Room B" \
  --attendees "alice@company.com,bob@company.com" \
  --description "Review Q1 milestones and blockers"
```

### Checking All Calendars

```bash
# List available calendars
ge calendar list --human

# Search across all calendars
ge calendar search "dentist" --limit 10
```

---

## Sync Management

### Checking Sync Status

```bash
# Check status for all accounts
ge sync status --human

# Check specific account
ge sync status --account work --human
```

### Syncing Older Emails

```bash
# Current sync goes back to 2022, extend to 2019
ge sync extend work --target-date 2019-01-01

# Restart daemon to apply (or wait for next sync cycle)
ge daemon restart
```

### Enabling Attachment Downloads

```bash
# Enable attachment downloads for account
ge account configure work --sync-attachments true

# Restart daemon to apply
ge daemon restart

# Download attachments for existing emails
ge sync download-attachments work
```

### Resetting Sync (Troubleshooting)

```bash
# If sync state is corrupted, reset and re-sync
ge sync reset work --type email --confirm

# Daemon will re-sync on next cycle
ge daemon restart
```

---

## Daemon Management

### Daily Operation

```bash
# Check daemon status
ge daemon status --human

# If not running, start it
ge daemon start --logging

# View recent logs (on macOS)
tail -f ~/.local/share/groundeffect/logs/daemon.log
```

### Changing Sync Frequency

```bash
# More frequent syncing (every 2 minutes for email)
ge daemon restart --email-poll 120 --calendar-poll 300

# Less frequent syncing (every 10 minutes)
ge daemon restart --email-poll 600 --calendar-poll 600
```

### Stopping for Maintenance

```bash
# Gracefully stop daemon
ge daemon stop

# Perform maintenance...

# Restart
ge daemon start --logging
```

---

## Troubleshooting

### Daemon Won't Start

```bash
# Check current status
ge daemon status

# If zombie process, find and kill it
ps aux | grep groundeffect
kill <pid>

# Then start fresh
ge daemon start --logging
```

### Missing Emails

```bash
# Check sync status
ge sync status --account affected-account --human

# If oldest_synced is recent, extend the range
ge sync extend affected-account --target-date 2020-01-01

# Force re-sync from date
# This will clear sync timestamps and re-fetch
ge sync reset affected-account --type email --confirm
```

### Account Authentication Issues

```bash
# Re-authenticate by removing and re-adding
ge account delete problematic@gmail.com --confirm
ge account add --alias work --years 3
```
