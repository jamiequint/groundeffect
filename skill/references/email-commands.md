# Email Commands Reference

## ge email search

Search emails using hybrid BM25 + vector semantic search.

```bash
ge email search "query" [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--from` | Filter by sender email/name | `--from "john@example.com"` |
| `--to` | Filter by recipient email/name | `--to "team@company.com"` |
| `--after` | Emails after date (YYYY-MM-DD) | `--after 2024-01-01` |
| `--before` | Emails before date (YYYY-MM-DD) | `--before 2024-12-31` |
| `--folder` | Filter by IMAP folder | `--folder INBOX` |
| `--has-attachment` | Only emails with attachments | `--has-attachment` |
| `--account` | Filter to specific account(s) | `--account work` |
| `--limit` | Number of results (1-100, default 10) | `--limit 25` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# Search for project updates from last month
ge email search "project status update" --after 2024-12-01

# Find emails from a specific sender with attachments
ge email search "invoice" --from "billing@vendor.com" --has-attachment

# Search across specific account only
ge email search "meeting notes" --account work --limit 20
```

---

## ge email list

List recent emails sorted by date (newest first). Faster than search for simple retrieval.

```bash
ge email list [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Filter to specific account | `--account personal` |
| `--limit` | Number of emails (1-100, default 10) | `--limit 50` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# List 10 most recent emails across all accounts
ge email list

# List 25 recent emails from work account
ge email list --account work --limit 25
```

---

## ge email show

Fetch a single email by ID with full content.

```bash
ge email show <id> [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--human` | Human-readable output |

### Output Fields
- `id` - Email unique identifier
- `from` - Sender address and name
- `to` - Recipient addresses
- `cc` - CC recipients
- `subject` - Email subject
- `date` - Send date/time
- `body` - Full email body (truncated at 40K chars)
- `attachments` - List of attachments with metadata
- `thread_id` - Gmail thread ID for threading
- `labels` - Gmail labels/IMAP folders

### Examples
```bash
# Show email by ID
ge email show abc123

# Show in human-readable format
ge email show abc123 --human
```

---

## ge email thread

Fetch all emails in a Gmail thread.

```bash
ge email thread <thread_id> [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Filter to specific accounts | `--account work,personal` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# Get full email thread
ge email thread 18abc123def

# Get thread with human-readable output
ge email thread 18abc123def --human
```

---

## ge email send

Compose and send an email. Uses preview workflow by default.

```bash
ge email send [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--to` | Recipient email address(es) | Yes |
| `--subject` | Email subject line | Yes |
| `--body` | Email body (plain text) | Yes |
| `--cc` | CC recipient(s) | No |
| `--bcc` | BCC recipient(s) | No |
| `--from-account` | Account to send from | No (uses default) |
| `--reply-to` | Email ID to reply to (for threading) | No |
| `--confirm` | Send immediately without preview | No |

### Examples
```bash
# Send with preview (returns preview, requires second call with --confirm)
ge email send --to "recipient@example.com" --subject "Hello" --body "Message body"

# Send immediately without preview
ge email send --to "recipient@example.com" --subject "Quick note" --body "Content" --confirm

# Reply to existing email
ge email send --to "sender@example.com" --subject "Re: Topic" --body "Reply content" --reply-to abc123

# Send with CC from specific account
ge email send --to "main@example.com" --cc "copy@example.com" --subject "Update" --body "..." --from-account work
```

---

## ge email attachment

Retrieve an email attachment.

```bash
ge email attachment <email_id> <filename> [options]
```

### Options
| Flag | Description |
|------|-------------|
| `--attachment-id` | Use attachment ID instead of filename |
| `--output` | Save to specific path |

### Output
- Text files: Returns text content directly
- Binary files: Returns local file path for access

### Examples
```bash
# Get attachment by filename
ge email attachment abc123 "document.pdf"

# Get attachment by ID
ge email attachment abc123 --attachment-id att_456
```

---

## ge email folders

List all IMAP folders/labels.

```bash
ge email folders [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Filter to specific account(s) | `--account work` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# List all folders across all accounts
ge email folders

# List folders for specific account
ge email folders --account personal
```
