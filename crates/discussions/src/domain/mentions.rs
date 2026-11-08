//! E-03: recognise selected identities and typed handles without trusting HTML.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MentionKey {
    User(i64),
    Handle(String),
    InvalidUserToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MentionToken {
    pub start: usize,
    pub end: usize,
    pub key: MentionKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MentionResolution {
    pub canonical_body: String,
    pub recipients: Vec<i64>,
}

fn handle_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
}

/// Ignore paired spoilers; they must never notify. UTF-8 offsets are only
/// emitted at ASCII token boundaries, so slicing the original String is safe.
pub fn tokens(body: &str) -> Vec<MentionToken> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut spoiler = false;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"||") {
            spoiler = !spoiler;
            i += 2;
            continue;
        }
        if spoiler || bytes[i] != b'@' {
            i += 1;
            continue;
        }
        if i > 0 && (handle_byte(bytes[i - 1]) || bytes[i - 1] == b'@') {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"@[user:") {
            let begin = i + 7;
            let end_digits = (begin..bytes.len())
                .take_while(|&j| bytes[j].is_ascii_digit())
                .last()
                .map_or(begin, |j| j + 1);
            if bytes.get(end_digits) == Some(&b']') {
                let digits = body.get(begin..end_digits).unwrap_or("");
                let key = if (1..=19).contains(&digits.len()) && !digits.starts_with('0') {
                    digits
                        .parse::<i64>()
                        .ok()
                        .filter(|id| *id > 0)
                        .map_or(MentionKey::InvalidUserToken, MentionKey::User)
                } else {
                    MentionKey::InvalidUserToken
                };
                out.push(MentionToken {
                    start: i,
                    end: end_digits + 1,
                    key,
                });
                i = end_digits + 1;
                continue;
            }
        }
        let begin = i + 1;
        let mut end = begin;
        while end < bytes.len() && handle_byte(bytes[end]) {
            end += 1;
        }
        if (3..=32).contains(&(end - begin)) {
            out.push(MentionToken {
                start: i,
                end,
                key: MentionKey::Handle(body[begin..end].to_ascii_lowercase()),
            });
        }
        i = if end > begin { end } else { i + 1 };
    }
    out
}

/// Resolve only server-authorised participants; ignore client-supplied IDs
/// unless the database verified the exact visible target in this book.
/// At most five distinct recipients may be added to one comment.
pub fn canonicalize(
    body: &str,
    verified: &[(MentionKey, i64)],
    actor_id: i64,
) -> MentionResolution {
    let mut result = String::with_capacity(body.len());
    let mut recipients = Vec::new();
    let mut cursor = 0;
    for mention in tokens(body) {
        result.push_str(&body[cursor..mention.start]);
        let candidate = verified
            .iter()
            .find(|(key, _)| *key == mention.key)
            .map(|(_, id)| *id);
        match candidate {
            Some(id) if id != actor_id && (recipients.contains(&id) || recipients.len() < 5) => {
                if !recipients.contains(&id) {
                    recipients.push(id);
                }
                result.push_str(&format!("@[user:{id}]"));
            }
            _ if matches!(mention.key, MentionKey::Handle(_)) && candidate.is_none() => {
                result.push_str(&body[mention.start..mention.end]);
            }
            _ => result.push_str("@пользователь"),
        }
        cursor = mention.end;
    }
    result.push_str(&body[cursor..]);
    MentionResolution {
        canonical_body: result,
        recipients,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_03_nonfriend_selected_id_is_preserved_only_when_server_verified() {
        let body = "Привет @[user:42], @[user:42] и @alice";
        let verified = [
            (MentionKey::User(42), 42),
            (MentionKey::Handle("alice".into()), 17),
        ];
        let result = canonicalize(body, &verified, 1);
        assert_eq!(
            result.canonical_body,
            "Привет @[user:42], @[user:42] и @[user:17]"
        );
        assert_eq!(result.recipients, vec![42, 17]);
        let attacker = canonicalize(body, &[], 1);
        assert_eq!(
            attacker.canonical_body,
            "Привет @пользователь, @пользователь и @alice"
        );
        assert!(attacker.recipients.is_empty());
    }

    #[test]
    fn e_03_spoilers_invalid_user_tokens_utf8_and_self_mentions_are_safe() {
        let text = "🙂 @[user:0] ||@[user:42] @alice|| @[user:42]";
        let result = canonicalize(text, &[(MentionKey::User(42), 42)], 42);
        assert_eq!(
            result.canonical_body,
            "🙂 @пользователь ||@[user:42] @alice|| @пользователь"
        );
        assert!(result.recipients.is_empty());
        assert_eq!(tokens("x@[user:42]"), Vec::new());
    }

    #[test]
    fn e_03_five_unique_recipients_limit_even_with_repeats() {
        let body = (1..=7)
            .map(|n| format!("@[user:{n}]"))
            .collect::<Vec<_>>()
            .join(" ");
        let verified = (1..=7)
            .map(|n| (MentionKey::User(n), n))
            .collect::<Vec<_>>();
        let result = canonicalize(&body, &verified, 99);
        assert_eq!(result.recipients.len(), 5);
        assert!(result
            .canonical_body
            .contains("@пользователь @пользователь"));
    }
}
