use super::*;
use lettre::transport::smtp::client::{Certificate, CertificateStore};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufStream},
    net::TcpListener,
    task::JoinHandle,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
        ServerConfig,
    },
    TlsAcceptor,
};

const CA: &[u8] = include_bytes!("fixtures/ca.pem");
const CERT: &[u8] = include_bytes!("fixtures/localhost.pem");
// Deliberately public, throwaway loopback-test key. Never a real credential.
const KEY: &[u8] = include_bytes!("fixtures/localhost-test-key.pem");

#[derive(Clone, Copy)]
enum Reply {
    Accept,
    AcceptWithoutId,
    RejectAuthentication,
    UnexpectedAuthenticationCode,
    TemporaryRecipient,
    PermanentData,
    LostAck,
    Stall,
    Probe,
}

fn transport(host: &str) -> PostboxTransport {
    PostboxTransport {
        sender: mailbox("sender@example.com").unwrap(),
        tls: TlsParameters::builder(host.to_owned())
            .certificate_store(CertificateStore::None)
            .add_root_certificate(Certificate::from_pem(CA).unwrap())
            .set_min_tls_version(TlsVersion::Tlsv12)
            .build_rustls()
            .unwrap(),
    }
}

fn credentials() -> PostboxCredentials {
    PostboxCredentials::new("ci-user".to_owned(), "ci-only".to_owned()).unwrap()
}

fn marker() -> String {
    format!("<tor-{}@pandaolate.ru>", "a".repeat(64))
}

async fn line<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut BufStream<S>,
) -> String {
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    line
}

async fn respond<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut BufStream<S>,
    bytes: &[u8],
) {
    stream.write_all(bytes).await.unwrap();
    stream.flush().await.unwrap();
}

async fn loopback(reply: Reply) -> (SocketAddr, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
    let cert = CertificateDer::from_pem_slice(CERT).unwrap();
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let Ok(socket) = TlsAcceptor::from(Arc::new(config)).accept(socket).await else {
            // The negative certificate test must never reach SMTP.
            return false;
        };
        let mut stream = BufStream::new(socket);
        respond(&mut stream, b"220 localhost ESMTP\r\n").await;
        assert!(line(&mut stream).await.starts_with("EHLO "));
        respond(&mut stream, b"250-localhost\r\n250 AUTH PLAIN\r\n").await;
        assert!(line(&mut stream).await.starts_with("AUTH PLAIN "));
        if matches!(reply, Reply::RejectAuthentication) {
            respond(&mut stream, b"535 5.7.8 Authentication rejected\r\n").await;
            return false;
        }
        if matches!(reply, Reply::UnexpectedAuthenticationCode) {
            respond(&mut stream, b"250 Unexpected reply\r\n").await;
            return false;
        }
        respond(&mut stream, b"235 2.7.0 Authenticated\r\n").await;
        if matches!(reply, Reply::Probe) {
            assert_eq!(line(&mut stream).await, "NOOP\r\n");
            respond(&mut stream, b"250 2.0.0 OK\r\n").await;
            return true;
        }
        assert!(line(&mut stream)
            .await
            .starts_with("MAIL FROM:<sender@example.com>"));
        respond(&mut stream, b"250 2.1.0 OK\r\n").await;
        assert!(line(&mut stream)
            .await
            .starts_with("RCPT TO:<reader@example.com>"));
        if matches!(reply, Reply::TemporaryRecipient) {
            respond(&mut stream, b"454 Sending rate exceeded\r\n").await;
            let _ = line(&mut stream).await;
            respond(&mut stream, b"221 Bye\r\n").await;
            return false;
        }
        respond(&mut stream, b"250 2.1.5 OK\r\n").await;
        assert_eq!(line(&mut stream).await, "DATA\r\n");
        respond(&mut stream, b"354 End data with dot\r\n").await;
        let mut data = String::new();
        loop {
            let next = line(&mut stream).await;
            assert!(!next.is_empty());
            if next == ".\r\n" {
                break;
            }
            data.push_str(&next);
        }
        assert!(data.contains(&marker()));
        assert!(data.contains("Content-Transfer-Encoding: base64"));
        assert!(data.is_ascii());
        match reply {
            Reply::Accept => {
                respond(&mut stream, b"250 2.6.0 OK: queued as receipt-123\r\n").await;
            }
            Reply::AcceptWithoutId => {
                respond(&mut stream, b"250 Accepted\r\n").await;
            }
            Reply::PermanentData => {
                respond(&mut stream, b"552 5.3.4 Message too big\r\n").await;
                let _ = line(&mut stream).await;
                respond(&mut stream, b"221 Bye\r\n").await;
            }
            Reply::LostAck => {}
            Reply::Stall => tokio::time::sleep(Duration::from_secs(1)).await,
            _ => unreachable!(),
        }
        // Abrupt connection close is intentional: a final 250 is sufficient.
        true
    });
    (address, task)
}

async fn send(reply: Reply) -> Submission {
    let (address, server) = loopback(reply).await;
    let transport = transport("localhost");
    let message = transport
        .prepare("reader@example.com", "Тест", "Письмо\n.строка", &marker())
        .unwrap();
    let connection = transport
        .connect_at(address, &credentials(), Duration::from_secs(3))
        .await
        .unwrap();
    let limit = if matches!(reply, Reply::Stall) {
        Duration::from_millis(150)
    } else {
        Duration::from_secs(3)
    };
    let result = connection.submit_with_limit(message, limit).await;
    timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap();
    result
}

#[test]
fn rejects_header_injection_invalid_markers_and_credentials() {
    let transport = PostboxTransport::new("sender@example.com").unwrap();
    assert!(matches!(
        transport.prepare(
            "a@example.com\r\nBcc:x@example.com",
            "Hi",
            "Body",
            &marker()
        ),
        Err(PrepareFailure::InvalidRecipient)
    ));
    assert!(matches!(
        transport.prepare(
            "a@example.com",
            "Hi\r\nBcc:x@example.com",
            "Body",
            &marker()
        ),
        Err(PrepareFailure::InvalidSubject)
    ));
    assert!(matches!(
        transport.prepare("a@example.com", "Hi", "Body", "<arbitrary@example.com>"),
        Err(PrepareFailure::InvalidMarker)
    ));
    assert!(matches!(
        PostboxCredentials::new("ci-user".to_owned(), "bad\nsecret".to_owned()),
        Err(ConfigurationError::InvalidCredentials)
    ));
    assert!(matches!(
        PostboxTransport::new("sender@example.com\n"),
        Err(ConfigurationError::InvalidSender)
    ));
}

#[tokio::test]
async fn smtps_acceptance_and_missing_id_are_not_retried() {
    assert_eq!(
        send(Reply::Accept).await,
        Submission::Accepted {
            provider_message_id: Some("receipt-123".to_owned())
        }
    );
    assert_eq!(
        send(Reply::AcceptWithoutId).await,
        Submission::Accepted {
            provider_message_id: None
        }
    );
}

#[tokio::test]
async fn definitive_rejections_are_distinct_from_ambiguous_submission() {
    assert_eq!(
        send(Reply::TemporaryRecipient).await,
        Submission::TemporaryRejected
    );
    assert_eq!(
        send(Reply::PermanentData).await,
        Submission::PermanentRejected
    );
    assert_eq!(send(Reply::LostAck).await, Submission::Uncertain);
    assert_eq!(send(Reply::Stall).await, Submission::Uncertain);
}

#[tokio::test]
async fn certificate_mismatch_and_authentication_failure_never_submit() {
    let (address, server) = loopback(Reply::Accept).await;
    assert!(matches!(
        transport("wrong.example.com")
            .connect_at(address, &credentials(), Duration::from_secs(3))
            .await,
        Err(ConnectFailure::Unavailable)
    ));
    assert!(!timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap());
    let (address, server) = loopback(Reply::RejectAuthentication).await;
    assert!(matches!(
        transport("localhost")
            .connect_at(address, &credentials(), Duration::from_secs(3))
            .await,
        Err(ConnectFailure::AuthenticationRejected)
    ));
    assert!(!timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap());
}

#[tokio::test]
async fn unexpected_positive_auth_reply_cannot_authorize_submission() {
    let (address, server) = loopback(Reply::UnexpectedAuthenticationCode).await;
    assert!(matches!(
        transport("localhost")
            .connect_at(address, &credentials(), Duration::from_secs(3))
            .await,
        Err(ConnectFailure::Unavailable)
    ));
    assert!(!timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap());
}

#[tokio::test]
async fn authenticated_probe_only_uses_noop() {
    let (address, server) = loopback(Reply::Probe).await;
    let mut session = transport("localhost")
        .connect_at(address, &credentials(), Duration::from_secs(3))
        .await
        .unwrap();
    assert!(timeout(PROBE_LIMIT, session.connection.test_connected())
        .await
        .unwrap());
    assert!(timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap());
}

#[tokio::test]
async fn password_reset_message_uses_the_same_authenticated_smtps_transport() {
    let (address, server) = loopback(Reply::Accept).await;
    let transport = transport("localhost");
    let origin = ResetOrigin::new("https://books.example").unwrap();
    let token = "A".repeat(43);
    let message = transport
        .prepare_reset_message(&origin, "reader@example.com", &token, &marker())
        .unwrap();
    let session = transport
        .connect_at(address, &credentials(), Duration::from_secs(3))
        .await
        .unwrap();
    let result = session
        .submit_with_limit(message, Duration::from_secs(3))
        .await;
    assert_eq!(result.terminal_outcome(), MailOutcome::Accepted);
    assert_eq!(result.accepted_id(), Some("receipt-123"));
    assert!(timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap());
}

#[test]
fn reset_delivery_classification_never_retries_uncertain_or_positive_ack() {
    assert_eq!(
        Submission::Uncertain.terminal_outcome(),
        MailOutcome::Uncertain
    );
    assert_eq!(
        Submission::TemporaryRejected.terminal_outcome(),
        MailOutcome::Failed
    );
    assert_eq!(
        Submission::PermanentRejected.terminal_outcome(),
        MailOutcome::Failed
    );
    assert_eq!(
        Submission::InvalidPreparedSender.terminal_outcome(),
        MailOutcome::Cancelled
    );
    let accepted = Submission::Accepted {
        provider_message_id: None,
    };
    assert_eq!(accepted.terminal_outcome(), MailOutcome::Accepted);
    assert_eq!(accepted.accepted_id(), None);
}
