//! MAIL 2/6: authenticated, fixed-endpoint Postbox SMTP over TLS.
//! This adapter never starts a worker, reads environment secrets or retries.
use crate::domain::postbox::{marker_token, valid_id};
use lettre::{
    message::{
        header::{ContentTransferEncoding, ContentType},
        Mailbox, SinglePart,
    },
    transport::smtp::{
        authentication::{Credentials, Mechanism},
        client::{AsyncSmtpConnection, TlsParameters, TlsVersion},
        extension::ClientId,
        response::Response,
    },
    Address, Message,
};
use std::time::Duration;
use tokio::{net::ToSocketAddrs, time::timeout};
use zeroize::Zeroizing;

pub const POSTBOX_HOST: &str = "postbox.cloud.yandex.net";
pub const POSTBOX_PORT: u16 = 465;
const CONNECT_LIMIT: Duration = Duration::from_secs(10);
const SUBMIT_LIMIT: Duration = Duration::from_secs(20);
const PROBE_LIMIT: Duration = Duration::from_secs(5);
const MAX_SUBJECT_BYTES: usize = 240;
const MAX_BODY_BYTES: usize = 65536;

/// Only redacted categories cross the transport boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationError {
    InvalidSender,
    InvalidCredentials,
    TlsUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectFailure {
    Unavailable,
    AuthenticationRejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareFailure {
    InvalidRecipient,
    InvalidSubject,
    InvalidBody,
    InvalidMarker,
    EncodingFailed,
}

/// Accepted is SMTP queue acceptance, not inbox delivery. A missing provider
/// ID never turns a positive final acknowledgement into a retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Submission {
    Accepted { provider_message_id: Option<String> },
    TemporaryRejected,
    PermanentRejected,
    InvalidPreparedSender,
    Uncertain,
}

/// No Debug/Serialize. The library necessarily makes short-lived credential
/// copies during AUTH; this wrapper zeroizes the caller-owned input buffers.
pub struct PostboxCredentials {
    key_id: Zeroizing<String>,
    secret: Zeroizing<String>,
}

impl PostboxCredentials {
    pub fn new(key_id: String, secret: String) -> Result<Self, ConfigurationError> {
        let key_id = Zeroizing::new(key_id);
        let secret = Zeroizing::new(secret);
        if !valid_credential(&key_id) || !valid_credential(&secret) {
            return Err(ConfigurationError::InvalidCredentials);
        }
        Ok(Self { key_id, secret })
    }
}

fn valid_credential(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn mailbox(value: &str) -> Option<Address> {
    if value.len() > 254 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    let (local, domain) = value.split_once('@')?;
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        || !domain.contains('.')
        || domain
            .rsplit('.')
            .next()
            .is_some_and(|suffix| suffix.bytes().all(|b| b.is_ascii_digit()))
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return None;
    }
    value.parse().ok()
}

pub(super) fn validate_content(
    recipient: &str,
    subject: &str,
    body: &str,
) -> Result<Address, PrepareFailure> {
    use PrepareFailure::*;
    let recipient = mailbox(recipient).ok_or(InvalidRecipient)?;
    if subject.is_empty()
        || subject.len() > MAX_SUBJECT_BYTES
        || subject.chars().any(char::is_control)
    {
        return Err(InvalidSubject);
    }
    if body.is_empty()
        || body.len() > MAX_BODY_BYTES
        || body
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
    {
        return Err(InvalidBody);
    }
    Ok(recipient)
}

/// Private message content is kept only in the current attempt, not logged
/// or serialized to the durable reference queue.
pub struct PreparedPostboxMessage {
    sender: Address,
    message: Message,
}

pub struct PostboxTransport {
    sender: Address,
    tls: TlsParameters,
}

impl PostboxTransport {
    /// No user-supplied host, port, roots, certificate bypass or plaintext.
    pub fn new(sender: &str) -> Result<Self, ConfigurationError> {
        let sender = mailbox(sender).ok_or(ConfigurationError::InvalidSender)?;
        let tls = TlsParameters::builder(POSTBOX_HOST.to_owned())
            .set_min_tls_version(TlsVersion::Tlsv12)
            .build_rustls()
            .map_err(|_| ConfigurationError::TlsUnavailable)?;
        Ok(Self { sender, tls })
    }

    /// The caller resolves the recipient and source event, and supplies the
    /// exact durable feedback marker allocated for this attempt.
    pub fn prepare(
        &self,
        recipient: &str,
        subject: &str,
        body: &str,
        marker: &str,
    ) -> Result<PreparedPostboxMessage, PrepareFailure> {
        use PrepareFailure::*;
        let recipient = validate_content(recipient, subject, body)?;
        if !marker.starts_with('<') || !marker.ends_with('>') || marker_token(marker).is_none() {
            return Err(InvalidMarker);
        }
        let message = Message::builder()
            .from(Mailbox::new(None, self.sender.clone()))
            .to(Mailbox::new(None, recipient))
            .subject(subject)
            .message_id(Some(marker.to_owned()))
            .singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_PLAIN)
                    .header(ContentTransferEncoding::Base64)
                    .body(body.to_owned()),
            )
            .map_err(|_| EncodingFailed)?;
        Ok(PreparedPostboxMessage {
            sender: self.sender.clone(),
            message,
        })
    }

    /// Connect and authenticate without sending any message. The worker must
    /// check the final DB/source/budget fence again before consuming the
    /// authenticated session in submit().
    pub async fn connect(
        &self,
        credentials: &PostboxCredentials,
    ) -> Result<AuthenticatedPostbox, ConnectFailure> {
        self.connect_at((POSTBOX_HOST, POSTBOX_PORT), credentials, CONNECT_LIMIT)
            .await
    }

    /// Authentication plus NOOP only. No MAIL FROM, RCPT TO or DATA.
    pub async fn probe(&self, credentials: &PostboxCredentials) -> Result<(), ConnectFailure> {
        let mut session = self.connect(credentials).await?;
        match timeout(PROBE_LIMIT, session.connection.test_connected()).await {
            Ok(true) => Ok(()),
            _ => Err(ConnectFailure::Unavailable),
        }
    }

    // Private endpoint injection exists solely for the loopback tests. TLS
    // remains mandatory even when testing the SMTP protocol.
    async fn connect_at<T: ToSocketAddrs>(
        &self,
        address: T,
        credentials: &PostboxCredentials,
        limit: Duration,
    ) -> Result<AuthenticatedPostbox, ConnectFailure> {
        let attempt = async {
            let hello = ClientId::Domain("pandaolate.ru".to_owned());
            let mut connection = AsyncSmtpConnection::connect_tokio1(
                address,
                Some(limit),
                &hello,
                Some(self.tls.clone()),
                None,
            )
            .await
            .map_err(|_| ConnectFailure::Unavailable)?;
            if !connection.is_encrypted() {
                return Err(ConnectFailure::Unavailable);
            }
            let auth = Credentials::new(
                credentials.key_id.to_string(),
                credentials.secret.to_string(),
            );
            let authenticated = connection
                .auth(&[Mechanism::Plain, Mechanism::Login], &auth)
                .await
                .map_err(|error| {
                    if error.is_permanent() {
                        ConnectFailure::AuthenticationRejected
                    } else {
                        ConnectFailure::Unavailable
                    }
                })?;
            if !authenticated.has_code(235) {
                return Err(ConnectFailure::Unavailable);
            }
            Ok(AuthenticatedPostbox {
                sender: self.sender.clone(),
                connection,
            })
        };
        timeout(limit, attempt)
            .await
            .unwrap_or(Err(ConnectFailure::Unavailable))
    }
}

/// Single-use session: submit consumes it; no implicit pooling or second
/// send. Dropping it closes the connection, including on timeout.
pub struct AuthenticatedPostbox {
    sender: Address,
    connection: AsyncSmtpConnection,
}

impl AuthenticatedPostbox {
    pub async fn submit(self, prepared: PreparedPostboxMessage) -> Submission {
        self.submit_with_limit(prepared, SUBMIT_LIMIT).await
    }

    async fn submit_with_limit(
        mut self,
        prepared: PreparedPostboxMessage,
        limit: Duration,
    ) -> Submission {
        if prepared.sender != self.sender {
            return Submission::InvalidPreparedSender;
        }
        let bytes = Zeroizing::new(prepared.message.formatted());
        match timeout(
            limit,
            self.connection.send(prepared.message.envelope(), &bytes),
        )
        .await
        {
            Ok(Ok(reply)) if reply.has_code(250) => Submission::Accepted {
                provider_message_id: accepted_message_id(&reply),
            },
            Ok(Err(error)) if error.is_transient() => Submission::TemporaryRejected,
            Ok(Err(error)) if error.is_permanent() => Submission::PermanentRejected,
            // Includes transport I/O, missing final reply, malformed reply,
            // unexpected positive/intermediate reply and total deadline.
            _ => Submission::Uncertain,
        }
        // Do not turn a successfully acknowledged message into a retry if
        // QUIT fails or the provider closes immediately after the final 250.
    }
}

fn accepted_message_id(reply: &Response) -> Option<String> {
    let line = reply.first_line()?;
    let token = line.strip_prefix("2.6.0 OK: queued as ")?;
    let token = token
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(token);
    valid_id(token).then(|| token.to_owned())
}

#[cfg(test)]
mod tests;
