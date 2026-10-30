//! I/G adapter for explicitly configured local MTA handoff. Never log reset secrets.
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(super) struct ResetDelivery {
    command: PathBuf,
    from: String,
    origin: String,
    permits: Arc<Semaphore>,
}

impl ResetDelivery {
    pub(super) fn from_env(origin: &str) -> Option<Self> {
        if std::env::var("TOR_PASSWORD_RESET_ENABLED").as_deref() != Ok("1") {
            return None;
        }
        let command = PathBuf::from(std::env::var("TOR_RESET_SENDMAIL").ok()?);
        let from = std::env::var("TOR_RESET_MAIL_FROM").ok()?;
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

    pub(super) fn reserve(&self) -> Option<OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }

    pub(super) async fn send(
        &self,
        recipient: String,
        token: String,
        permit: OwnedSemaphorePermit,
    ) -> bool {
        if !safe_mailbox(&recipient) {
            return false;
        }
        let config = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut child = match Command::new(&config.command).args(["-i", "-t"])
                .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                    Ok(child) => child, Err(_) => return false,
                };
            // Fragment is consumed locally by the reset form, never sent in an HTTP URL.
            let message = format!("From: {}\r\nTo: {}\r\nSubject: TOR password reset\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nДля смены пароля откройте ссылку (действует 30 минут):\r\n{}/reset-password#{}\r\n\r\nЕсли вы не запрашивали смену, проигнорируйте письмо.\r\n", config.from, recipient, config.origin, token);
            let written = child.stdin.take().is_some_and(|mut input| input.write_all(message.as_bytes()).is_ok());
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
        }).await.unwrap_or(false)
    }
}

fn safe_mailbox(value: &str) -> bool {
    value.len() <= 254
        && value.contains('@')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_07_mail_headers_cannot_inject_recipients_or_flags() {
        assert!(safe_mailbox("reader+test@example.invalid"));
        for value in [
            "x@example.invalid\r\nBcc: evil@example.invalid",
            "--help",
            "a <b@example.invalid>",
        ] {
            assert!(!safe_mailbox(value));
        }
    }
}
