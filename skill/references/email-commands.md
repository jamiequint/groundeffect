# Email Commands Reference

## groundeffect email search

Search emails using hybrid BM25 + vector semantic search.

```bash
groundeffect email search "query" [options]
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
groundeffect email search "project status update" --after 2024-12-01

# Find emails from a specific sender with attachments
groundeffect email search "invoice" --from "billing@vendor.com" --has-attachment

# Search across specific account only
groundeffect email search "meeting notes" --account work --limit 20
```

---

## groundeffect email list

List recent emails sorted by date (newest first). Faster than search for simple retrieval.

```bash
groundeffect email list [options]
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
groundeffect email list

# List 25 recent emails from work account
groundeffect email list --account work --limit 25
```

---

## groundeffect email show

Fetch a single email by ID with full content.

```bash
groundeffect email show <id> [options]
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
groundeffect email show abc123

# Show in human-readable format
groundeffect email show abc123 --human
```

---

## groundeffect email thread

Fetch all emails in a Gmail thread.

```bash
groundeffect email thread <thread_id> [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Filter to specific accounts | `--account work,personal` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# Get full email thread
groundeffect email thread 18abc123def

# Get thread with human-readable output
groundeffect email thread 18abc123def --human
```

---

## groundeffect email send

Compose and send an email. Uses preview workflow by default. Supports HTML emails with automatic content detection.

```bash
groundeffect email send [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--to` | Recipient email address(es) | Yes |
| `--subject` | Email subject line | Yes |
| `--body` | Email body (plain text or HTML) | Yes |
| `--cc` | CC recipient(s) | No |
| `--bcc` | BCC recipient(s) | No |
| `--from-account` | Account to send from | No (uses default) |
| `--reply-to` | Email ID to reply to (for threading) | No |
| `--html` | Force HTML email mode | No |
| `--save-as-draft` | Save as draft instead of sending | No |
| `--confirm` | Send immediately without preview | No |

### HTML Email Support
- **Auto-detection**: Content is automatically detected as HTML if it contains HTML tags, markdown links, or URLs
- **Markdown conversion**: Markdown-style formatting (links, bold, italic) is converted to HTML
- **Multipart format**: HTML emails are sent as multipart/alternative with plain text fallback
- **Force HTML**: Use `--html` flag to ensure HTML processing even for simple content

### Examples
```bash
# Send with preview (returns preview, requires second call with --confirm)
groundeffect email send --to "recipient@example.com" --subject "Hello" --body "Message body"

# Send immediately without preview
groundeffect email send --to "recipient@example.com" --subject "Quick note" --body "Content" --confirm

# Send HTML email (auto-detected from content)
groundeffect email send --to "recipient@example.com" --subject "Report" --body "<h1>Monthly Report</h1><p>Here are the details...</p>" --confirm

# Send with markdown links (auto-converted to HTML)
groundeffect email send --to "recipient@example.com" --subject "Links" --body "Check out [this link](https://example.com)" --confirm

# Force HTML mode
groundeffect email send --to "recipient@example.com" --subject "Update" --body "Simple text" --html --confirm

# Save as draft instead of sending
groundeffect email send --to "recipient@example.com" --subject "Draft" --body "Work in progress" --save-as-draft

# Reply to existing email
groundeffect email send --to "sender@example.com" --subject "Re: Topic" --body "Reply content" --reply-to abc123

# Send with CC from specific account
groundeffect email send --to "main@example.com" --cc "copy@example.com" --subject "Update" --body "..." --from-account work
```

---

## groundeffect email attachment

Retrieve an email attachment.

```bash
groundeffect email attachment <email_id> <filename> [options]
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
groundeffect email attachment abc123 "document.pdf"

# Get attachment by ID
groundeffect email attachment abc123 --attachment-id att_456
```

---

## groundeffect email folders

List all IMAP folders/labels.

```bash
groundeffect email folders [options]
```

### Options
| Flag | Description | Example |
|------|-------------|---------|
| `--account` | Filter to specific account(s) | `--account work` |
| `--human` | Human-readable output | `--human` |

### Examples
```bash
# List all folders across all accounts
groundeffect email folders

# List folders for specific account
groundeffect email folders --account personal
```

---

## groundeffect email draft create

Create a new email draft.

```bash
groundeffect email draft create --from <account> --to <email> --subject "X" --body "X" [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--from` | Account to create draft in (email or alias) | Yes |
| `--to` | Recipient email address(es) | Yes |
| `--subject` | Email subject line | Yes |
| `--body` | Email body (plain text or HTML) | Yes |
| `--cc` | CC recipient(s) | No |
| `--bcc` | BCC recipient(s) | No |
| `--html` | Force HTML email mode | No |
| `--reply-to` | Email ID to reply to (for threading) | No |
| `--human` | Human-readable output | No |

### Examples
```bash
# Create a simple draft
groundeffect email draft create --from work --to "recipient@example.com" --subject "Draft email" --body "Working on this..."

# Create an HTML draft
groundeffect email draft create --from work --to "team@example.com" --subject "Report" --body "<h1>Report</h1>" --html

# Create a draft reply to an existing email thread
groundeffect email draft create --from work --to "sender@example.com" --subject "Re: Topic" --body "Reply draft" --reply-to abc123
```

---

## groundeffect email draft list

List email drafts for an account.

```bash
groundeffect email draft list --from <account> [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--from` | Account to list drafts from (email or alias) | Yes |
| `--limit` | Number of drafts (default 20) | No |
| `--human` | Human-readable output | No |

### Examples
```bash
# List drafts from work account
groundeffect email draft list --from work

# List more drafts with human-readable output
groundeffect email draft list --from work --limit 50 --human
```

---

## groundeffect email draft show

Get details of a specific draft.

```bash
groundeffect email draft show --from <account> --draft-id <id> [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--from` | Account the draft belongs to (email or alias) | Yes |
| `--draft-id` | Draft ID to show | Yes |
| `--human` | Human-readable output | No |

### Examples
```bash
# Show draft details
groundeffect email draft show --from work --draft-id r123456789

# Show in human-readable format
groundeffect email draft show --from work --draft-id r123456789 --human
```

---

## groundeffect email draft update

Update an existing draft. Only provided fields are updated; omitted fields keep their current values.

```bash
groundeffect email draft update --from <account> --draft-id <id> [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--from` | Account the draft belongs to (email or alias) | Yes |
| `--draft-id` | Draft ID to update | Yes |
| `--to` | Update recipient(s) | No |
| `--subject` | Update subject line | No |
| `--body` | Update body content | No |
| `--cc` | Update CC recipient(s) | No |
| `--bcc` | Update BCC recipient(s) | No |
| `--html` | Force HTML email mode | No |
| `--human` | Human-readable output | No |

### Examples
```bash
# Update draft subject and body
groundeffect email draft update --from work --draft-id r123456789 --subject "Updated Subject" --body "New content"

# Add CC to existing draft
groundeffect email draft update --from work --draft-id r123456789 --cc "copy@example.com"
```

---

## groundeffect email draft send

Send an existing draft.

```bash
groundeffect email draft send --from <account> --draft-id <id> [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--from` | Account the draft belongs to (email or alias) | Yes |
| `--draft-id` | Draft ID to send | Yes |
| `--human` | Human-readable output | No |

### Examples
```bash
# Send a draft
groundeffect email draft send --from work --draft-id r123456789

# Send with human-readable output
groundeffect email draft send --from work --draft-id r123456789 --human
```

---

## groundeffect email draft delete

Delete a draft permanently.

```bash
groundeffect email draft delete --from <account> --draft-id <id> [options]
```

### Options
| Flag | Description | Required |
|------|-------------|----------|
| `--from` | Account the draft belongs to (email or alias) | Yes |
| `--draft-id` | Draft ID to delete | Yes |
| `--human` | Human-readable output | No |

### Examples
```bash
# Delete a draft
groundeffect email draft delete --from work --draft-id r123456789
```
