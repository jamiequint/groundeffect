# Common Workflows

## Initial Setup

### Adding Your First Account

```bash
# Add a Google account with 3 years of history
groundeffect account add --alias work --years 3

# Browser opens for OAuth - authorize GroundEffect
# Once authorized, sync begins automatically

# Check sync progress
groundeffect sync status --account work --human

# Start daemon for continuous sync
groundeffect daemon start --logging
```

### Adding Multiple Accounts

```bash
# Add work account
groundeffect account add --alias work --years 5

# Add personal account
groundeffect account add --alias personal --years 2

# List all accounts
groundeffect account list --human
```

---

## Email Workflows

### Finding Recent Emails from a Person

```bash
# Search for emails from a specific sender
groundeffect email search "from:john@company.com" --limit 20 --human

# Or use the --from flag
groundeffect email search "" --from "john@company.com" --limit 20 --human
```

### Searching for Emails in a Date Range

```bash
# Find project-related emails from last month
groundeffect email search "project update" --after 2024-12-01 --before 2024-12-31

# Find emails with attachments from Q4
groundeffect email search "report" --after 2024-10-01 --has-attachment --limit 50
```

### Reading an Email Thread

```bash
# First, search for the email
groundeffect email search "contract negotiation" --limit 5

# Get the thread_id from the result, then fetch full thread
groundeffect email thread 18abc123def456 --human
```

### Downloading an Attachment

```bash
# Show email to see attachments
groundeffect email show email_abc123 --human

# Download specific attachment
groundeffect email attachment email_abc123 "contract.pdf"
```

### Sending an Email

```bash
# Preview the email first (default behavior)
groundeffect email send \
  --to "recipient@example.com" \
  --subject "Project Update" \
  --body "Hi, here is the latest project status..."

# Review the preview, then send with confirmation
groundeffect email send \
  --to "recipient@example.com" \
  --subject "Project Update" \
  --body "Hi, here is the latest project status..." \
  --confirm
```

### Replying to an Email

```bash
# Get the email ID from search or list
groundeffect email show original_email_id

# Send reply (uses --reply-to for threading)
groundeffect email send \
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
groundeffect calendar search "meeting" --after 2024-01-15 --before 2024-01-22 --human
```

### Creating a Meeting

```bash
# Create a simple meeting
groundeffect calendar create \
  --summary "Team Standup" \
  --start "2024-01-16T09:00:00" \
  --end "2024-01-16T09:30:00"

# Create meeting with attendees and location
groundeffect calendar create \
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
groundeffect calendar list --human

# Search across all calendars
groundeffect calendar search "dentist" --limit 10
```

---

## Sync Management

### Checking Sync Status

```bash
# Check status for all accounts
groundeffect sync status --human

# Check specific account
groundeffect sync status --account work --human
```

### Syncing Older Emails

```bash
# Current sync goes back to 2022, extend to 2019
groundeffect sync extend work --target-date 2019-01-01

# Restart daemon to apply (or wait for next sync cycle)
groundeffect daemon restart
```

### Enabling Attachment Downloads

```bash
# Enable attachment downloads for account
groundeffect account configure work --sync-attachments true

# Restart daemon to apply
groundeffect daemon restart

# Download attachments for existing emails
groundeffect sync download-attachments work
```

### Resetting Sync (Troubleshooting)

```bash
# If sync state is corrupted, reset and re-sync
groundeffect sync reset work --type email --confirm

# Daemon will re-sync on next cycle
groundeffect daemon restart
```

---

## Daemon Management

### Daily Operation

```bash
# Check daemon status
groundeffect daemon status --human

# If not running, start it
groundeffect daemon start --logging

# View recent logs (on macOS)
tail -f ~/.local/share/groundeffect/logs/daemon.log
```

### Changing Sync Frequency

```bash
# More frequent syncing (every 2 minutes for email)
groundeffect daemon restart --email-poll 120 --calendar-poll 300

# Less frequent syncing (every 10 minutes)
groundeffect daemon restart --email-poll 600 --calendar-poll 600
```

### Stopping for Maintenance

```bash
# Gracefully stop daemon
groundeffect daemon stop

# Perform maintenance...

# Restart
groundeffect daemon start --logging
```

---

## Troubleshooting

### Daemon Won't Start

```bash
# Check current status
groundeffect daemon status

# If zombie process, find and kill it
ps aux | grep groundeffect
kill <pid>

# Then start fresh
groundeffect daemon start --logging
```

### Missing Emails

```bash
# Check sync status
groundeffect sync status --account affected-account --human

# If oldest_synced is recent, extend the range
groundeffect sync extend affected-account --target-date 2020-01-01

# Force re-sync from date
# This will clear sync timestamps and re-fetch
groundeffect sync reset affected-account --type email --confirm
```

### Account Authentication Issues

```bash
# Re-authenticate by removing and re-adding
groundeffect account delete problematic@gmail.com --confirm
groundeffect account add --alias work --years 3
```
