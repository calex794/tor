//! E-03: parse bounded user mentions. Never trust a client-supplied stable ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MentionToken {
    pub start: usize,
    pub end: usize,
    pub user_id: i64,
}

/// Client tokens are accepted only as *candidates* for server-side authorization.
/// Invalid tokens remain ordinary text. Mentions within paired spoilers are ignored.
pub fn candidates(body: &str) -> Vec<MentionToken> {
    let mut out = Vec::new();
    let mut spoiler = false;
    let mut i = 0;
    let bytes = body.as_bytes();
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
        if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || b"_@".contains(&bytes[i - 1])) {
            i += 1;
            continue;
        }
        let Some(tail) = body.get(i + 1..) else {
            i += 1;
            continue;
        };
        let Some(digits) = tail.strip_prefix("[user:") else {
            i += 1;
            continue;
        };
        let len = digits.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=19).contains(&len) || digits.starts_with('0') || digits.as_bytes().get(len) != Some(&b']') {
            i += 1;
            continue;
        }
        let end = i + 1 + 6 + len + 1;
        if bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.') {
            i += 1;
            continue;
        }
        if let Ok(user_id) = digits[..len].parse::<i64>() {
            if user_id > 0 && !out.iter().any(|x: &MentionToken| x.user_id == user_id) {
                out.push(MentionToken { start: i, end, user_id });
            }
        }
        i = end;
        if out.len() >= 5 {
            break;
        }
    }
    out
}

/// Unknown or disallowed references are replaced with neutral markup. This is
/// always called before persistence; a user cannot forge an arbitrary recipient.
pub fn canonicalize(body: &str, allowed: &[i64]) -> String {
    let tokens = candidates(body);
    let mut output = String::new();
    let mut start = 0;
    for token in tokens {
        output.push_str(&body[start..token.start]);
        if allowed.contains(&token.user_id) {
            output.push_str(&body[token.start..token.end]);
        } else {
            output.push_str("@пользователь");
        }
        start = token.end;
    }
    output.push_str(&body[start..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_03_structured_tokens_are_deduplicated_and_bounded() {
        assert_eq!(candidates("hey @[user:42] @[user:42] @[user:17]").iter().map(|x| x.user_id).collect::<Vec<_>>(), vec![42,17]);
        assert_eq!(candidates("a@[user:42] @[user:0] @[user:42]x"), Vec::new());
        assert_eq!(candidates("||@[user:42]|| @[user:17]").iter().map(|x| x.user_id).collect::<Vec<_>>(), vec![17]);
        assert_eq!(canonicalize("hello @[user:42] and @[user:17]", &[17]), "hello @пользователь and @[user:17]");
    }
}
