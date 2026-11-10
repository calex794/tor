//! Dedicated opt-in email proof MTA: strictly separate from password reset.
//! No content delivery, chapter data, URLs with secrets or automatic sending.
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(super) struct ProofDelivery {
    command: PathBuf,
    from: String,
    origin: String,
    permits: Arc<Semaphore>,
}

impl ProofDelivery {
    pub(super) fn from_env(origin: &str) -> Option<Self> {
        // No existing TOR_RESET_* setting can enable this adapter.
        if std::env::var("TOR_NOTIFICATION_PROOF_ENABLED").as_deref() != Ok("1") {
            return None;
        }
        let command = PathBuf::from(std::env::var("TOR_NOTIFICATION_PROOF_SENDMAIL").ok()?);
        let from = std::env::var("TOR_NOTIFICATION_PROOF_FROM").ok()?;
        if !command.is_absolute()
            || !command.is_file()
            || !safe_mailbox(&from)
            || !(origin.starts_with("https://") || origin.starts_with("http://127.0.0.1:"))
            || origin.chars().any(char::is_whitespace)
            || origin.contains(['#', '?', '\r', '\n'])
        {
            return None;
        }
        Some(Self {
            command,
            from,
            origin: origin.trim_end_matches('/').to_owned(),
            permits: Arc::new(Semaphore::new(4)),
        })
    }

    pub(super) fn mailbox_allowed(&self, email: &str) -> bool {
        safe_mailbox(email)
    }

    pub(super) fn reserve(&self) -> Option<OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }

    pub(super) async fn send(
        &self,
        recipient: String,
        proof_code: String,
        permit: OwnedSemaphorePermit,
    ) -> bool {
        if !safe_mailbox(&recipient) || !crate::auth_cookies::valid_token(&proof_code) {
            return false;
        }
        let sender = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut child = match Command::new(&sender.command)
                .args(["-i", "-t"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(_) => return false,
            };
            // Copy-only code: never place proof in an HTTP URL, query,
            // fragment, error log or tracing event.
            let message = format!(
                "From: {}\r\nTo: {}\r\nSubject: TOR email verification code\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nВы запросили подтверждение email для будущих уведомлений о главах.\r\n\r\nКод (30 минут): {}\r\n\r\nОткройте страницу {} /me/notification-settings и вставьте код в форму. Если вы не запрашивали код, ничего не делайте.\r\n",
                sender.from,
                recipient,
                proof_code,
                sender.origin
            );
            let written = child
                .stdin
                .take()
                .is_some_and(|mut stdin| stdin.write_all(message.as_bytes()).is_ok());
            let deadline = Instant::now() + Duration::from_secs(5);
            if written {
                while Instant::now() < deadline {
                    match child.try_wait() {
                        Ok(Some(status)) => return status.success(),
                        Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                        Err(_) => break,
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            false
        })
        .await
        .unwrap_or(false)
    }
}

fn safe_mailbox(value: &str) -> bool {
    value.len() <= 254
        && value.contains('@')
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_proof_headers_reject_injection_and_local_flags() {
        assert!(safe_mailbox("reader+test@example.invalid"));
        for bad in [
            "x@example.invalid\r\nBcc: somebody@example.invalid",
            "-t",
            "a <another@example.invalid>",
        ] {
            assert!(!safe_mailbox(bad));
        }
    }
}
