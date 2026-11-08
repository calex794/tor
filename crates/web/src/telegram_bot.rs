//! Default-off Telegram Bot API long-polling adapter for A/S004.
//! Browser-initiated recovery only. Reset secrets never go to Telegram.
mod recovery;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::task::JoinHandle;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};

#[derive(Clone)]
struct BotConfig {
    token: String,
    origin: String,
}

#[derive(Deserialize)]
struct BotResponse {
    ok: bool,
    result: Vec<BotUpdate>,
}
#[derive(Deserialize)]
struct BotUpdate {
    update_id: i64,
    message: Option<BotMessage>,
    callback_query: Option<BotCallback>,
}

#[derive(Deserialize)]
struct BotCallback {
    id: String,
    #[serde(rename = "from")]
    from: BotUser,
    message: Option<BotCallbackMessage>,
    data: Option<String>,
}
#[derive(Deserialize)]
struct BotCallbackMessage {
    chat: BotChat,
}
#[derive(Deserialize)]
struct BotMessage {
    #[serde(rename = "from")]
    from: Option<BotUser>,
    chat: BotChat,
    text: Option<String>,
}
#[derive(Deserialize)]
struct BotUser {
    id: i64,
    is_bot: bool,
}
#[derive(Deserialize)]
struct BotChat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(PartialEq, Eq, Debug)]
enum BotCommand<'a> {
    Bind(&'a str),
    Recover,
    None,
}

fn parse_command(text: &str) -> BotCommand<'_> {
    if text == "/recover" || text == "/start" {
        return BotCommand::Recover;
    }
    match text.strip_prefix("/start bind_") {
        Some(secret)
            if secret.len() == 43
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            BotCommand::Bind(secret)
        }
        _ => BotCommand::None,
    }
}

fn validated_bot_username() -> Result<String, &'static str> {
    let value = std::env::var("TOR_TELEGRAM_BOT_USERNAME")
        .map_err(|_| "TOR_TELEGRAM_BOT_USERNAME missing")?;
    if !(5..=32).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || !value.to_ascii_lowercase().ends_with("bot")
    {
        return Err("Invalid TOR_TELEGRAM_BOT_USERNAME");
    }
    Ok(value)
}

fn config(origin: &str) -> Result<Option<BotConfig>, &'static str> {
    match std::env::var("TOR_TELEGRAM_RECOVERY_ENABLED").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("0") => return Ok(None),
        Ok("1") => {}
        _ => return Err("TOR_TELEGRAM_RECOVERY_ENABLED must be 0 or 1"),
    }
    let token =
        std::env::var("TOR_TELEGRAM_BOT_TOKEN").map_err(|_| "TOR_TELEGRAM_BOT_TOKEN missing")?;
    let (bot_id, secret) = token.split_once(':').ok_or("Invalid Telegram bot token")?;
    if bot_id.is_empty()
        || !bot_id.bytes().all(|b| b.is_ascii_digit())
        || !(25..=160).contains(&secret.len())
        || !secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || !(origin.starts_with("https://") || origin.starts_with("http://127.0.0.1:"))
        || origin.contains(['#', '?', '\r', '\n', ' ', '\t'])
    {
        return Err("Invalid Telegram recovery configuration");
    }
    let _username = validated_bot_username()?;
    Ok(Some(BotConfig {
        token,
        origin: origin.trim_end_matches('/').to_owned(),
    }))
}

pub async fn start_from_pending(
    pending: Option<(PgAccounts, String)>,
) -> Result<Option<JoinHandle<()>>, &'static str> {
    if let Some((accounts, origin)) = pending {
        start(accounts, origin).await
    } else {
        if std::env::var("TOR_TELEGRAM_RECOVERY_ENABLED").as_deref() == Ok("1") {
            return Err("Telegram recovery requires authenticated PostgreSQL, not demo");
        }
        Ok(None)
    }
}

/// Starts only after HTTP listener binding and a probe of the new schema.
/// Exactly one operator-enabled bot poller may consume this bot token.
pub async fn start(
    accounts: PgAccounts,
    origin: String,
) -> Result<Option<JoinHandle<()>>, &'static str> {
    let Some(cfg) = config(&origin)? else {
        return Ok(None);
    };
    accounts
        .probe_telegram_recovery()
        .await
        .map_err(|_| "Telegram recovery migration is missing")?;
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "Telegram HTTP client unavailable")?;
    Ok(Some(tokio::spawn(async move {
        poll_forever(client, accounts, cfg).await;
    })))
}

async fn send_message(client: &Client, endpoint: &str, chat_id: i64, text: &str) -> bool {
    if chat_id <= 0 || text.len() > 3500 {
        return false;
    }
    let response = client
        .post(endpoint)
        .json(&json!({
            "chat_id":chat_id,
            "text":text,
            "link_preview_options":{"is_disabled":true},
        }))
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {
            response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|result| result.get("ok").and_then(|v| v.as_bool()))
                == Some(true)
        }
        _ => false,
    }
}

async fn process_message(
    client: &Client,
    send_endpoint: &str,
    accounts: &PgAccounts,
    cfg: &BotConfig,
    message: BotMessage,
) {
    let Some(sender) = message.from else {
        return;
    };
    if sender.is_bot
        || sender.id <= 0
        || message.chat.kind != "private"
        || message.chat.id != sender.id
    {
        return;
    }
    let Some(text) = message.text else {
        return;
    };
    match parse_command(text.trim()) {
        BotCommand::Bind(nonce) => {
            let result = accounts
                .claim_telegram_link(&session_token_digest(nonce), sender.id, message.chat.id)
                .await;
            let reply = if matches!(result, Ok(true)) {
                format!("Telegram ID: {}. Вернитесь в настройки TOR, проверьте этот ID и подтвердите привязку.", sender.id)
            } else {
                "Ссылка привязки недействительна или устарела. Запросите новую в TOR.".to_owned()
            };
            let _ = send_message(client, send_endpoint, sender.id, &reply).await;
        }
        BotCommand::Recover => {
            // A bot-started command cannot create a reset credential. The
            // browser must initiate the challenge and retain its private proof.
            let reply = format!(
                "Для восстановления пароля TOR откройте {}/recover/telegram \
                 в своём браузере. Проверьте код на сайте и подтвердите заявку здесь. \
                 Пароль и одноразовые ссылки боту не отправляйте.",
                cfg.origin
            );
            let _ = send_message(client, send_endpoint, sender.id, &reply).await;
        }
        BotCommand::None => {}
    }
}

async fn poll_forever(client: Client, accounts: PgAccounts, cfg: BotConfig) {
    // Fixed Telegram authority; never allow an operator-supplied endpoint.
    let poll = format!("https://api.telegram.org/bot{}/getUpdates", cfg.token);
    let send = format!("https://api.telegram.org/bot{}/sendMessage", cfg.token);
    let answer = format!(
        "https://api.telegram.org/bot{}/answerCallbackQuery",
        cfg.token
    );
    let mut offset = 0_i64;
    loop {
        recovery::dispatch(&client, &send, &accounts).await;
        let response = client
            .post(&poll)
            .json(&json!({
                "offset":offset,"limit":25,"timeout":20,
                "allowed_updates":["message","callback_query"]
            }))
            .send()
            .await;
        let parsed = match response {
            Ok(response) if response.status().is_success() => {
                response.json::<BotResponse>().await.ok()
            }
            _ => None,
        };
        let Some(response) = parsed.filter(|res| res.ok) else {
            // Never log token, URLs, usernames, request bodies or Telegram IDs.
            eprintln!("TOR_TELEGRAM_POLL_UNAVAILABLE");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };
        let mut storage_failure = false;
        for update in response.result {
            // Never advance the polling offset when the idempotency write
            // failed. Otherwise an unrecorded approval could be lost.
            let duplicate = match accounts.mark_telegram_update(update.update_id).await {
                Ok(true) => false,
                Ok(false) => true,
                Err(_) => {
                    storage_failure = true;
                    break;
                }
            };
            offset = offset.max(update.update_id.saturating_add(1));
            if duplicate {
                continue;
            }
            if let Some(message) = update.message {
                process_message(&client, &send, &accounts, &cfg, message).await;
            }
            if let Some(callback) = update.callback_query {
                recovery::callback(&client, &answer, &accounts, &cfg, callback).await;
            }
        }
        if storage_failure {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_07_bot_ignores_forged_commands() {
        assert_eq!(parse_command("/recover"), BotCommand::Recover);
        assert_eq!(parse_command("/start"), BotCommand::Recover);
        assert_eq!(parse_command("/start bind_short"), BotCommand::None);
        assert_eq!(parse_command("/recover please"), BotCommand::None);
        assert_eq!(parse_command("/start bind_abc\n/recover"), BotCommand::None);
        let nonce = generate_session_token();
        assert_eq!(
            parse_command(&format!("/start bind_{nonce}")),
            BotCommand::Bind(nonce.as_str())
        );
    }
}
