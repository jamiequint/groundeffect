//! IMAP client for Gmail with XOAUTH2 authentication

use std::sync::Arc;

use async_imap::{Authenticator, Client as ImapClientAsync};
use async_native_tls::TlsConnector;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use mail_parser::MimeHeaders;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::models::{Address, Attachment, Email};
use crate::oauth::OAuthManager;

use super::{GlobalRateLimiter, SyncEvent};

/// Retry configuration
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 1000;
const MAX_RETRY_DELAY_MS: u64 = 30000;

/// XOAUTH2 authenticator for IMAP
struct XOAuth2Auth {
    auth_string: String,
}

impl Authenticator for XOAuth2Auth {
    type Response = String;

    fn process(&mut self, challenge: &[u8]) -> Self::Response {
        let response = std::mem::take(&mut self.auth_string);
        info!("XOAUTH2 process called, challenge len: {}, response len: {}",
               challenge.len(), response.len());
        response
    }
}

/// Type alias for the IMAP session with our TLS stream
type ImapSession = async_imap::Session<async_native_tls::TlsStream<tokio_util::compat::Compat<TcpStream>>>;

/// Gmail IMAP settings
const IMAP_HOST: &str = "imap.gmail.com";
const IMAP_PORT: u16 = 993;

/// IMAP client for a single account
pub struct ImapClient {
    account_id: String,
    oauth: Arc<OAuthManager>,
    rate_limiter: Arc<GlobalRateLimiter>,
}

impl ImapClient {
    /// Create a new IMAP client
    pub async fn new(
        account_id: &str,
        oauth: Arc<OAuthManager>,
        rate_limiter: Arc<GlobalRateLimiter>,
    ) -> Result<Self> {
        Ok(Self {
            account_id: account_id.to_string(),
            oauth,
            rate_limiter,
        })
    }

    /// Connect to Gmail IMAP with retry logic
    async fn connect_with_retry(&self) -> Result<ImapSession> {
        let mut last_error = None;
        let mut delay_ms = INITIAL_RETRY_DELAY_MS;

        for attempt in 1..=MAX_RETRIES {
            match self.connect().await {
                Ok(session) => return Ok(session),
                Err(e) => {
                    warn!(
                        "IMAP connection attempt {}/{} failed for {}: {}",
                        attempt, MAX_RETRIES, self.account_id, e
                    );
                    last_error = Some(e);

                    if attempt < MAX_RETRIES {
                        info!("Retrying in {}ms...", delay_ms);
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        delay_ms = (delay_ms * 2).min(MAX_RETRY_DELAY_MS);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Imap("Connection failed after retries".to_string())))
    }

    /// Connect to Gmail IMAP
    async fn connect(&self) -> Result<ImapSession> {
        self.rate_limiter.wait().await;

        info!("Connecting to Gmail IMAP for {}...", self.account_id);

        info!("  Step 1: Opening TCP connection to {}:{}...", IMAP_HOST, IMAP_PORT);
        let tcp = TcpStream::connect((IMAP_HOST, IMAP_PORT))
            .await
            .map_err(|e| Error::ConnectionFailed {
                host: IMAP_HOST.to_string(),
                reason: e.to_string(),
            })?;
        info!("  Step 1: TCP connection established");

        // Wrap TCP stream with compat layer for futures AsyncRead/Write
        let tcp_compat = tcp.compat();

        info!("  Step 2: Establishing TLS connection...");
        let tls = TlsConnector::new();
        let tls_stream = tls.connect(IMAP_HOST, tcp_compat).await.map_err(|e| {
            Error::ConnectionFailed {
                host: IMAP_HOST.to_string(),
                reason: e.to_string(),
            }
        })?;
        info!("  Step 2: TLS connection established");

        let mut client = ImapClientAsync::new(tls_stream);

        // Gmail's IMAP server sends a greeting before accepting commands
        // We must read it before authenticating (see async-imap #84)
        info!("  Step 2.5: Reading server greeting...");
        match client.read_response().await {
            Some(Ok(_greeting)) => {
                info!("  Step 2.5: Server greeting received");
            }
            Some(Err(e)) => {
                return Err(Error::Imap(format!("Failed to read greeting: {:?}", e)));
            }
            None => {
                return Err(Error::Imap("Unexpected end of stream, expected greeting".to_string()));
            }
        }

        // Get fresh access token
        info!("  Step 3: Getting access token...");
        let access_token = self.oauth.get_valid_token(&self.account_id).await?;
        info!("  Step 3: Got access token");

        // Generate XOAUTH2 string
        let auth_string = OAuthManager::generate_xoauth2(&self.account_id, &access_token);

        // Authenticate with timeout
        info!("  Step 4: Authenticating with XOAUTH2...");
        let auth = XOAuth2Auth { auth_string };
        let auth_future = client.authenticate("XOAUTH2", auth);
        let session = tokio::time::timeout(std::time::Duration::from_secs(30), auth_future)
            .await
            .map_err(|_| Error::Imap("XOAUTH2 authentication timed out after 30s".to_string()))?
            .map_err(|(e, _)| Error::Imap(format!("Authentication failed: {:?}", e)))?;

        info!("  Step 4: Authentication successful!");
        info!("Connected to Gmail IMAP for {}", self.account_id);
        Ok(session)
    }

    /// Count total emails in INBOX (for progress estimation)
    pub async fn count_emails(&self) -> Result<u64> {
        let mut session = self.connect().await?;

        // Select INBOX and get message count
        let mailbox = session
            .select("INBOX")
            .await
            .map_err(|e| Error::Imap(format!("Failed to select INBOX: {:?}", e)))?;

        let count = mailbox.exists as u64;
        session.logout().await.ok();

        Ok(count)
    }

    /// Count emails since a specific date using IMAP SEARCH
    pub async fn count_emails_since(&self, since: DateTime<Utc>) -> Result<u64> {
        let mut session = self.connect().await?;

        // Select INBOX
        session
            .select("INBOX")
            .await
            .map_err(|e| Error::Imap(format!("Failed to select INBOX: {:?}", e)))?;

        // Search for emails since the target date
        let since_str = since.format("%d-%b-%Y").to_string();
        let search_query = format!("SINCE {}", since_str);

        let uids = session
            .uid_search(&search_query)
            .await
            .map_err(|e| Error::Imap(format!("Failed to search emails: {:?}", e)))?;

        let count = uids.len() as u64;
        session.logout().await.ok();

        Ok(count)
    }

    /// Fetch all emails in a date range, newest first, with automatic reconnection on failure
    /// Returns emails in batches via callback to allow incremental processing
    /// If `before` is Some, fetches emails SINCE `since` AND BEFORE `before` (for backfill)
    /// If `before` is None, fetches all emails SINCE `since` (for incremental sync)
    pub async fn fetch_all_emails_since<F, Fut>(
        &self,
        since: DateTime<Utc>,
        before: Option<DateTime<Utc>>,
        batch_size: usize,
        mut on_batch: F,
    ) -> Result<usize>
    where
        F: FnMut(Vec<Email>) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        // Avoid panic in slice chunking if config is set to 0.
        let batch_size = batch_size.max(1);

        // Connect with retry
        let mut session = self.connect_with_retry().await?;

        // Select INBOX
        if let Err(e) = session.select("INBOX").await {
            warn!("First select INBOX failed, reconnecting: {:?}", e);
            session = self.connect_with_retry().await?;
            session
                .select("INBOX")
                .await
                .map_err(|e| Error::Imap(format!("Failed to select INBOX after reconnect: {:?}", e)))?;
        }

        // Search for emails in date range
        // Add 2-day buffer to before date to catch emails at boundaries and timezone edge cases
        let since_str = since.format("%d-%b-%Y").to_string();
        let search_query = if let Some(before_date) = before {
            let before_with_buffer = before_date + chrono::Duration::days(2);
            let before_str = before_with_buffer.format("%d-%b-%Y").to_string();
            format!("SINCE {} BEFORE {}", since_str, before_str)
        } else {
            format!("SINCE {}", since_str)
        };

        self.rate_limiter.wait().await;
        let uids = session
            .uid_search(&search_query)
            .await
            .map_err(|e| Error::Imap(format!("Search failed: {:?}", e)))?;

        // Collect UIDs and sort descending (newest first - higher UID = newer in Gmail)
        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_by(|a, b| b.cmp(a)); // Descending order

        let total_count = uids.len();
        if let Some(before_date) = before {
            info!("Found {} emails from {} to {} for {}", total_count, since_str, before_date.format("%d-%b-%Y"), self.account_id);
        } else {
            info!("Found {} emails since {} for {}", total_count, since_str, self.account_id);
        }

        if uids.is_empty() {
            session.logout().await.ok();
            return Ok(0);
        }

        // Fetch in batches, with reconnection on failure
        let mut total_fetched = 0;
        let mut batch_index = 0;
        let uid_batches: Vec<Vec<u32>> = uids.chunks(batch_size).map(|c| c.to_vec()).collect();

        while batch_index < uid_batches.len() {
            let uid_batch = &uid_batches[batch_index];
            let uid_range = uid_batch
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",");

            self.rate_limiter.wait().await;

            // Try to fetch the batch - we need to handle reconnection carefully
            // to avoid borrow checker issues with the session
            let fetches: Vec<_> = {
                use futures::StreamExt;
                match session
                    .uid_fetch(&uid_range, "(UID FLAGS ENVELOPE BODY.PEEK[] X-GM-MSGID X-GM-THRID X-GM-LABELS)")
                    .await
                {
                    Ok(messages) => messages.collect().await,
                    Err(e) => {
                        warn!("Fetch failed for batch {}, will reconnect: {:?}", batch_index, e);
                        vec![] // Empty vec signals we need to reconnect
                    }
                }
            };

            // If fetch failed (empty result), reconnect and retry
            let fetches = if fetches.is_empty() && !uid_batch.is_empty() {
                // Reconnect with retry
                session = match self.connect_with_retry().await {
                    Ok(s) => s,
                    Err(reconnect_err) => {
                        error!("Failed to reconnect after fetch error: {}", reconnect_err);
                        return Err(reconnect_err);
                    }
                };

                // Re-select INBOX
                if let Err(select_err) = session.select("INBOX").await {
                    error!("Failed to re-select INBOX: {:?}", select_err);
                    return Err(Error::Imap(format!("Failed to re-select INBOX: {:?}", select_err)));
                }

                // Retry this batch
                self.rate_limiter.wait().await;
                use futures::StreamExt;
                match session
                    .uid_fetch(&uid_range, "(UID FLAGS ENVELOPE BODY.PEEK[] X-GM-MSGID X-GM-THRID X-GM-LABELS)")
                    .await
                {
                    Ok(messages) => messages.collect().await,
                    Err(retry_err) => {
                        error!("Fetch still failed after reconnect: {:?}", retry_err);
                        // Skip this batch and continue with the next one
                        warn!("Skipping batch {} ({} emails) due to persistent error", batch_index, uid_batch.len());
                        batch_index += 1;
                        continue;
                    }
                }
            } else {
                fetches
            };

            // Parse emails, handling individual failures gracefully
            let mut emails = Vec::new();
            let mut parse_errors = 0;
            for result in fetches {
                match result {
                    Ok(fetch) => {
                        match self.parse_fetch(&fetch) {
                            Ok(Some(email)) => emails.push(email),
                            Ok(None) => {} // No email parsed (missing UID or body)
                            Err(e) => {
                                parse_errors += 1;
                                debug!("Failed to parse email: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        parse_errors += 1;
                        debug!("Error in fetch stream: {:?}", e);
                    }
                }
            }

            if parse_errors > 0 {
                warn!("Skipped {} emails due to parse errors in batch {}", parse_errors, batch_index);
            }

            // Sort by date descending
            emails.sort_by(|a, b| b.date.cmp(&a.date));

            let batch_count = emails.len();
            total_fetched += batch_count;

            info!("Fetched batch of {} emails ({}/{}) for {}", batch_count, total_fetched, total_count, self.account_id);

            // Call the callback with this batch
            if !emails.is_empty() {
                if let Err(e) = on_batch(emails).await {
                    // Log error but continue - emails will be refetched on next sync resume
                    error!("Batch {} callback failed, will be retried on next sync: {}", batch_index, e);
                }
            }

            batch_index += 1;
        }

        session.logout().await.ok();
        Ok(total_fetched)
    }

    /// Fetch emails newest first with pagination (offset and limit) - legacy method
    pub async fn fetch_emails_newest_first(
        &self,
        since: DateTime<Utc>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Email>> {
        let mut session = self.connect().await?;

        // Select INBOX
        session
            .select("INBOX")
            .await
            .map_err(|e| Error::Imap(format!("Failed to select INBOX: {:?}", e)))?;

        // Search for emails since the given date
        let since_str = since.format("%d-%b-%Y").to_string();
        let search_query = format!("SINCE {}", since_str);

        self.rate_limiter.wait().await;
        let uids = session
            .uid_search(&search_query)
            .await
            .map_err(|e| Error::Imap(format!("Search failed: {:?}", e)))?;

        // Collect UIDs and sort descending (newest first - higher UID = newer in Gmail)
        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_by(|a, b| b.cmp(a)); // Descending order

        // Apply offset and limit
        let uids: Vec<u32> = uids.into_iter().skip(offset).take(limit).collect();
        debug!("Found {} emails since {} (offset={}, limit={})", uids.len(), since_str, offset, limit);

        if uids.is_empty() {
            session.logout().await.ok();
            return Ok(vec![]);
        }

        // Fetch email data
        let uid_range = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        self.rate_limiter.wait().await;
        let messages = session
            .uid_fetch(&uid_range, "(UID FLAGS ENVELOPE BODY.PEEK[] X-GM-MSGID X-GM-THRID X-GM-LABELS)")
            .await
            .map_err(|e| Error::Imap(format!("Fetch failed: {:?}", e)))?;

        // Collect all messages into a vector to release the borrow on session
        use futures::StreamExt;
        let fetches: Vec<_> = messages.collect().await;

        // Now we can logout
        session.logout().await.ok();

        // Parse the collected messages and preserve order (newest first)
        let mut emails = Vec::new();
        for result in fetches {
            match result {
                Ok(fetch) => {
                    if let Some(email) = self.parse_fetch(&fetch)? {
                        emails.push(email);
                    }
                }
                Err(e) => {
                    warn!("Error fetching message: {:?}", e);
                }
            }
        }

        // Sort by date descending to ensure newest first
        emails.sort_by(|a, b| b.date.cmp(&a.date));

        info!("Fetched {} emails for {} (newest first)", emails.len(), self.account_id);
        Ok(emails)
    }

    /// Fetch recent emails since a given date
    pub async fn fetch_recent_emails(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Email>> {
        // Delegate to newest-first with offset 0
        self.fetch_emails_newest_first(since, limit, 0).await
    }

    /// Parse a fetched message into an Email struct
    fn parse_fetch(&self, fetch: &async_imap::types::Fetch) -> Result<Option<Email>> {
        let uid = match fetch.uid {
            Some(uid) => uid,
            None => return Ok(None),
        };

        let body = match fetch.body() {
            Some(b) => b,
            None => return Ok(None),
        };

        // Parse email using mail-parser
        let parsed = mail_parser::MessageParser::default()
            .parse(body)
            .ok_or_else(|| Error::InvalidEmailFormat("Failed to parse email".to_string()))?;

        let message_id = parsed
            .message_id()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("<{}@unknown>", uid));

        let from = parsed
            .from()
            .and_then(|addrs| addrs.first())
            .map(|addr| Address {
                name: addr.name().map(|s| s.to_string()),
                email: addr.address().map(|s| s.to_string()).unwrap_or_default(),
            })
            .unwrap_or_else(|| Address::new("unknown@unknown.com"));

        let to: Vec<Address> = parsed
            .to()
            .map(|addrs| {
                addrs
                    .iter()
                    .map(|addr| Address {
                        name: addr.name().map(|s| s.to_string()),
                        email: addr.address().map(|s| s.to_string()).unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let cc: Vec<Address> = parsed
            .cc()
            .map(|addrs| {
                addrs
                    .iter()
                    .map(|addr| Address {
                        name: addr.name().map(|s| s.to_string()),
                        email: addr.address().map(|s| s.to_string()).unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let subject = parsed.subject().unwrap_or("(No Subject)").to_string();

        let date = parsed
            .date()
            .map(|d| DateTime::from_timestamp(d.to_timestamp(), 0).unwrap_or_else(Utc::now))
            .unwrap_or_else(Utc::now);

        let body_plain = parsed
            .body_text(0)
            .map(|s| s.to_string())
            .unwrap_or_default();

        let body_html = parsed.body_html(0).map(|s| s.to_string());

        let snippet = body_plain.chars().take(200).collect();

        // Parse attachments
        let attachments: Vec<Attachment> = parsed
            .attachments()
            .map(|att| {
                Attachment::new(
                    uuid::Uuid::new_v4().to_string(),
                    att.attachment_name().unwrap_or("attachment"),
                    att.content_type()
                        .map(|ct| ct.ctype().to_string())
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    att.len() as u64,
                )
            })
            .collect();

        // Parse flags
        let flags: Vec<String> = fetch
            .flags()
            .map(|f| format!("{:?}", f))
            .collect();

        // Gmail extensions
        let gmail_message_id = fetch.uid.unwrap_or(0) as u64; // TODO: Parse X-GM-MSGID
        let gmail_thread_id = fetch.uid.unwrap_or(0) as u64;  // TODO: Parse X-GM-THRID

        let in_reply_to = parsed.in_reply_to().as_text().map(|s| s.to_string());
        let references: Vec<String> = parsed
            .references()
            .as_text_list()
            .map(|list| list.into_iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();

        // Use a stable ID based on account + message_id to prevent duplicates on re-sync
        let stable_id = format!("{}:{}", self.account_id, &message_id);

        let email = Email {
            id: stable_id,
            account_id: self.account_id.clone(),
            account_alias: None,
            message_id,
            gmail_message_id,
            gmail_thread_id,
            uid,
            in_reply_to,
            references,
            folder: "INBOX".to_string(),
            labels: vec![],
            flags,
            from,
            to,
            cc,
            bcc: vec![],
            subject,
            date,
            body_plain,
            body_html,
            snippet,
            attachments,
            embedding: None,
            synced_at: Utc::now(),
            raw_size: body.len() as u64,
        };

        Ok(Some(email))
    }

    /// Download a specific attachment from an email
    /// Returns the attachment content as bytes
    pub async fn download_attachment(
        &self,
        uid: u32,
        attachment_filename: &str,
    ) -> Result<(Vec<u8>, String)> {
        info!(
            "Downloading attachment '{}' from email UID {} for {}",
            attachment_filename, uid, self.account_id
        );

        let mut session = self.connect_with_retry().await?;

        // Select INBOX (or we could make folder configurable)
        session
            .select("INBOX")
            .await
            .map_err(|e| Error::Imap(format!("Failed to select INBOX: {:?}", e)))?;

        // Fetch the full email body
        let fetch_result = session
            .uid_fetch(uid.to_string(), "BODY[]")
            .await
            .map_err(|e| Error::Imap(format!("Failed to fetch email UID {}: {:?}", uid, e)))?;

        let fetches: Vec<_> = fetch_result.collect::<Vec<_>>().await;

        if fetches.is_empty() {
            return Err(Error::Imap(format!("Email UID {} not found", uid)));
        }

        let fetch = fetches
            .into_iter()
            .next()
            .ok_or_else(|| Error::Imap(format!("No fetch result for UID {}", uid)))?
            .map_err(|e| Error::Imap(format!("Fetch error: {:?}", e)))?;

        let body = fetch
            .body()
            .ok_or_else(|| Error::Imap("No body in fetch".to_string()))?;

        // Parse the email
        let parsed = mail_parser::MessageParser::default()
            .parse(body)
            .ok_or_else(|| Error::Imap("Failed to parse email".to_string()))?;

        // Find the attachment by filename
        for att in parsed.attachments() {
            let name = att.attachment_name().unwrap_or("attachment");
            if name == attachment_filename {
                let content = att.contents().to_vec();
                let mime_type = att
                    .content_type()
                    .map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream")))
                    .unwrap_or_else(|| "application/octet-stream".to_string());

                info!(
                    "Downloaded attachment '{}' ({} bytes, {})",
                    attachment_filename,
                    content.len(),
                    mime_type
                );

                let _ = session.logout().await;
                return Ok((content, mime_type));
            }
        }

        let _ = session.logout().await;
        Err(Error::Imap(format!(
            "Attachment '{}' not found in email UID {}",
            attachment_filename, uid
        )))
    }

    /// Download all attachments from an email and save to disk
    /// Returns a list of (attachment_id, local_path) for successfully downloaded attachments
    pub async fn download_all_attachments(
        &self,
        uid: u32,
        attachments_dir: &std::path::Path,
    ) -> Result<Vec<(String, std::path::PathBuf, String, u64)>> {
        info!(
            "Downloading all attachments from email UID {} for {}",
            uid, self.account_id
        );

        let mut session = self.connect_with_retry().await?;

        // Select INBOX
        session
            .select("INBOX")
            .await
            .map_err(|e| Error::Imap(format!("Failed to select INBOX: {:?}", e)))?;

        // Fetch the full email body
        let fetch_result = session
            .uid_fetch(uid.to_string(), "BODY[]")
            .await
            .map_err(|e| Error::Imap(format!("Failed to fetch email UID {}: {:?}", uid, e)))?;

        let fetches: Vec<_> = fetch_result.collect::<Vec<_>>().await;

        if fetches.is_empty() {
            let _ = session.logout().await;
            return Ok(vec![]);
        }

        let fetch = match fetches.into_iter().next() {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                let _ = session.logout().await;
                return Err(Error::Imap(format!("Fetch error: {:?}", e)));
            }
            None => {
                let _ = session.logout().await;
                return Ok(vec![]);
            }
        };

        let body = match fetch.body() {
            Some(b) => b,
            None => {
                let _ = session.logout().await;
                return Ok(vec![]);
            }
        };

        // Parse the email
        let parsed = match mail_parser::MessageParser::default().parse(body) {
            Some(p) => p,
            None => {
                let _ = session.logout().await;
                return Ok(vec![]);
            }
        };

        let mut downloaded = Vec::new();

        // Create account-specific subdirectory
        let account_dir = attachments_dir.join(&self.account_id);
        std::fs::create_dir_all(&account_dir)?;

        // Download each attachment
        for att in parsed.attachments() {
            let filename = att.attachment_name().unwrap_or("attachment").to_string();
            let content = att.contents();
            let size = content.len() as u64;

            // Skip large attachments (50MB limit)
            if size > 50 * 1024 * 1024 {
                warn!(
                    "Skipping attachment '{}' ({} bytes) - exceeds 50MB limit",
                    filename, size
                );
                continue;
            }

            let mime_type = att
                .content_type()
                .map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream")))
                .unwrap_or_else(|| "application/octet-stream".to_string());

            // Generate unique filename to avoid collisions
            let unique_id = uuid::Uuid::new_v4().to_string();
            let safe_filename = sanitize_filename(&filename);
            let local_filename = format!("{}_{}", &unique_id[..8], safe_filename);
            let local_path = account_dir.join(&local_filename);

            // Write to disk
            if let Err(e) = std::fs::write(&local_path, content) {
                error!("Failed to write attachment '{}': {}", filename, e);
                continue;
            }

            debug!(
                "Saved attachment '{}' to {:?} ({} bytes)",
                filename, local_path, size
            );

            downloaded.push((unique_id, local_path, mime_type, size));
        }

        let _ = session.logout().await;
        info!(
            "Downloaded {} attachments from email UID {}",
            downloaded.len(),
            uid
        );

        Ok(downloaded)
    }

    /// Start IMAP IDLE for real-time notifications
    pub async fn start_idle(&self, event_tx: mpsc::Sender<SyncEvent>) -> Result<()> {
        loop {
            let mut session = match self.connect().await {
                Ok(s) => s,
                Err(e) => {
                    error!("IDLE connection failed: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
            };

            if let Err(e) = session.select("INBOX").await {
                error!("Failed to select INBOX for IDLE: {:?}", e);
                continue;
            }

            info!("Starting IDLE for {}", self.account_id);

            // Enter IDLE mode
            let mut idle_handle = session.idle();

            // Initialize IDLE - sends the IDLE command to server
            if let Err(e) = idle_handle.init().await {
                error!("Failed to init IDLE: {:?}", e);
                continue;
            }

            // Wait for IDLE response (timeout after 29 minutes - Gmail's limit is 30 min)
            // wait_with_timeout returns (Future, StopSource) - we await the future
            let (wait_future, _stop_source) = idle_handle.wait_with_timeout(std::time::Duration::from_secs(29 * 60));
            match wait_future.await {
                Ok(_) => {
                    // Got a notification, fetch new emails
                    let _ = event_tx
                        .send(SyncEvent::NewEmail {
                            account_id: self.account_id.clone(),
                            email_id: String::new(),
                        })
                        .await;
                }
                Err(e) => {
                    debug!("IDLE wait error (may be timeout): {:?}", e);
                }
            }

            // Done with IDLE, session will be dropped and reconnected

            // Reconnect after IDLE ends
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

/// Sanitize a filename to be safe for the filesystem
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}
