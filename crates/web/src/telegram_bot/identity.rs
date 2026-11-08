//! A/S004 startup checks. Refuse wrong bot identity or a conflicting webhook.
//! Never log the Bot API token, URL or identity response body.
use super::BotConfig;
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize};

#[derive(Deserialize)]
struct ApiEnvelope<T> {
    ok: bool,
    result: Option<T>,
}

#[derive(Deserialize)]
struct BotIdentity {
    id: i64,
    is_bot: bool,
    username: Option<String>,
}

#[derive(Deserialize)]
struct WebhookInfo {
    url: String,
}

fn valid_identity(config: &BotConfig, result: &BotIdentity) -> bool {
    let Some((id, _)) = config.token.split_once(':') else {
        return false;
    };
    let Ok(id) = id.parse::<i64>() else {
        return false;
    };
    result.is_bot
        && result.id == id
        && result
            .username
            .as_deref()
            .is_some_and(|username| username.eq_ignore_ascii_case(&config.username))
}

fn polling_allowed(webhook: &WebhookInfo) -> bool {
    webhook.url.is_empty()
}

async fn get_result<T: DeserializeOwned>(
    client: &Client,
    token: &str,
    method: &str,
) -> Result<T, &'static str> {
    // The method is a fixed internal constant, not supplied by an HTTP user.
    let endpoint = format!("https://api.telegram.org/bot{token}/{method}");
    let response = client
        .get(endpoint)
        .send()
        .await
        .map_err(|_| "Telegram Bot API identity check unavailable")?;
    if !response.status().is_success() {
        return Err("Telegram Bot API identity check rejected");
    }
    let envelope: ApiEnvelope<T> = response
        .json()
        .await
        .map_err(|_| "Telegram Bot API identity check invalid response")?;
    if !envelope.ok {
        return Err("Telegram Bot API identity check rejected");
    }
    envelope
        .result
        .ok_or("Telegram Bot API identity check missing result")
}

pub(super) async fn verify(client: &Client, cfg: &BotConfig) -> Result<(), &'static str> {
    let info: BotIdentity = get_result(client, &cfg.token, "getMe").await?;
    if !valid_identity(cfg, &info) {
        return Err("Telegram bot token and username identify different bots");
    }
    let webhook: WebhookInfo = get_result(client, &cfg.token, "getWebhookInfo").await?;
    if !polling_allowed(&webhook) {
        return Err("Telegram webhook is active; long polling is unavailable");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BotConfig {
        BotConfig {
            token: format!("123456:{}", "a".repeat(35)),
            origin: "https://books.example".to_owned(),
            username: "TorRecoveryBot".to_owned(),
        }
    }

    fn identity(id: i64, name: Option<&str>, is_bot: bool) -> BotIdentity {
        BotIdentity {
            id,
            is_bot,
            username: name.map(str::to_owned),
        }
    }

    #[test]
    fn a_07_bot_identity_must_match_both_token_number_and_declared_username() {
        let cfg = config();
        assert!(valid_identity(
            &cfg,
            &identity(123456, Some("torrecoverybot"), true)
        ));
        assert!(!valid_identity(
            &cfg,
            &identity(123457, Some("TorRecoveryBot"), true)
        ));
        assert!(!valid_identity(
            &cfg,
            &identity(123456, Some("UnrelatedBot"), true)
        ));
        assert!(!valid_identity(&cfg, &identity(123456, None, true)));
        assert!(!valid_identity(
            &cfg,
            &identity(123456, Some("TorRecoveryBot"), false)
        ));
    }

    #[test]
    fn a_07_long_polling_refuses_a_preexisting_webhook() {
        assert!(polling_allowed(&WebhookInfo { url: String::new() }));
        assert!(!polling_allowed(&WebhookInfo {
            url: "https://webhook.example/callback".to_owned(),
        }));
    }

    #[test]
    fn a_07_bot_identity_and_webhook_api_responses_parse_without_logging_secrets() {
        let good: ApiEnvelope<BotIdentity> = serde_json::from_str(
            r#"{"ok":true,"result":{"id":123456,"is_bot":true,"username":"TorRecoveryBot"}}"#,
        )
        .unwrap();
        assert!(good.ok && good.result.is_some());
        let missing: ApiEnvelope<BotIdentity> =
            serde_json::from_str(r#"{"ok":false,"description":"unauthorized"}"#).unwrap();
        assert!(!missing.ok && missing.result.is_none());
        let webhook: ApiEnvelope<WebhookInfo> =
            serde_json::from_str(r#"{"ok":true,"result":{"url":""}}"#).unwrap();
        assert!(webhook.ok && webhook.result.is_some_and(|info| polling_allowed(&info)));
    }
}
