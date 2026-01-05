//! IMAP client for Gmail with XOAUTH2 authentication

use std::sync::Arc;

use async_imap::{Authenticator, Client as ImapClientAsync};
use async_native_tls::TlsConnector;
use chrono::{DateTime, Utc};
use mail_parser::MimeHeaders;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::models::{Address, Attachment, Email};
use crate::oauth::OAuthManager;

use super::{GlobalRateLimiter, SyncEvent};

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

    /// Fetch all emails since a date, newest first, in a single connection
    /// Returns emails in batches via callback to allow incremental processing
    pub async fn fetch_all_emails_since<F, Fut>(
        &self,
        since: DateTime<Utc>,
        batch_size: usize,
        mut on_batch: F,
    ) -> Result<usize>
    where
        F: FnMut(Vec<Email>) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
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

        let total_count = uids.len();
        info!("Found {} emails since {} for {}", total_count, since_str, self.account_id);

        if uids.is_empty() {
            session.logout().await.ok();
            return Ok(0);
        }

        // Fetch in batches, keeping the same connection
        let mut total_fetched = 0;
        for uid_batch in uids.chunks(batch_size) {
            let uid_range = uid_batch
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",");

            self.rate_limiter.wait().await;
            let messages = session
                .uid_fetch(&uid_range, "(UID FLAGS ENVELOPE BODY.PEEK[] X-GM-MSGID X-GM-THRID X-GM-LABELS)")
                .await
                .map_err(|e| Error::Imap(format!("Fetch failed: {:?}", e)))?;

            // Collect messages
            use futures::StreamExt;
            let fetches: Vec<_> = messages.collect().await;

            // Parse emails
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

            // Sort by date descending
            emails.sort_by(|a, b| b.date.cmp(&a.date));

            let batch_count = emails.len();
            total_fetched += batch_count;

            info!("Fetched batch of {} emails ({}/{}) for {}", batch_count, total_fetched, total_count, self.account_id);

            // Call the callback with this batch
            on_batch(emails).await?;
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
