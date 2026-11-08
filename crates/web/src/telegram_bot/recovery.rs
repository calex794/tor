//! Telegram Bot API approval UI: comparison code only, no password reset secrets.
use super::{BotCallback, BotConfig};
use reqwest::Client;
use serde_json::json;
use tor_accounts::repositories::PgAccounts;

fn decode_callback(data: &str) -> Option<(i64, bool)> {
    let (action, id) = data.rsplit_once(':')?;
    let approve = match action {
        "tr:a" => true,
        "tr:d" => false,
        _ => return None,
    };
    if id.is_empty() || id.len() > 18 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let request_id = id.parse::<i64>().ok()?;
    (request_id > 0).then_some((request_id, approve))
}

pub(super) async fn dispatch(client: &Client, send_endpoint: &str, accounts: &PgAccounts) {
    let Ok(queued) = accounts.pending_telegram_recovery_notifications().await else {
        return;
    };
    for request in queued {
        let text = format!(
            "TOR: запрошена смена пароля.\nКод сверки: {}.\n\
             Сверьте его с кодом на странице TOR. Действует 5 минут.\n\
             Если вы не начинали восстановление, нажмите «Отклонить».",
            request.match_code
        );
        let keyboard = json!({
            "inline_keyboard":[
                [{"text":"Подтвердить","callback_data":format!("tr:a:{}", request.id)}],
                [{"text":"Отклонить","callback_data":format!("tr:d:{}", request.id)}]
            ]
        });
        let sent = client
            .post(send_endpoint)
            .json(&json!({
                "chat_id":request.private_chat_id,
                "text":text,
                "reply_markup":keyboard,
                "link_preview_options":{"is_disabled":true}
            }))
            .send()
            .await;
        let delivered = match sent {
            Ok(reply) if reply.status().is_success() => {
                reply
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|value| value.get("ok").and_then(|v| v.as_bool()))
                    == Some(true)
            }
            _ => false,
        };
        if delivered {
            let _ = accounts.mark_telegram_recovery_notified(request.id).await;
        }
    }
}

pub(super) async fn callback(
    client: &Client,
    answer_endpoint: &str,
    accounts: &PgAccounts,
    _config: &BotConfig,
    cb: BotCallback,
) -> Result<(), sqlx::Error> {
    let Some(message) = cb.message else {
        return Ok(());
    };
    if cb.from.is_bot
        || cb.from.id <= 0
        || message.chat.kind != "private"
        || message.chat.id != cb.from.id
    {
        return Ok(());
    }
    let action = cb.data.as_deref().and_then(decode_callback);
    let changed = if let Some((id, approve)) = action {
        accounts
            .decide_telegram_recovery(id, cb.from.id, message.chat.id, approve)
            .await?
    } else {
        false
    };
    let response = if changed {
        "Решение принято. Вернитесь на ту же страницу TOR и обновите её."
    } else {
        "Заявка уже обработана, недействительна или устарела."
    };
    // A callback acknowledgement has no reset authority.
    let _ = client
        .post(answer_endpoint)
        .json(&json!({
            "callback_query_id":cb.id,
            "text":response,
            "show_alert":false
        }))
        .send()
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_07_callback_data_is_not_a_bearer_secret_or_freeform_command() {
        assert_eq!(decode_callback("tr:a:123"), Some((123, true)));
        assert_eq!(decode_callback("tr:d:123"), Some((123, false)));
        for candidate in [
            "tr:a:0",
            "tr:a:-1",
            "tr:a:99999999999999999999999",
            "tr:x:5",
            "tr:a:5:6",
            "tr:a:abc",
            "tr:a:1\ntr:a:2",
        ] {
            assert_eq!(decode_callback(candidate), None);
        }
    }
}
