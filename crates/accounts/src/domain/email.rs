//! Public-site mailbox policy, separate from legacy account identity lookup.
//! This is syntax validation only; no network or claim of mailbox existence.
use super::normalize_email;

const INVALID: &str = "Некорректный адрес почты";

/// Keep the historic normalized account key. Convert only the transport
/// domain when checking syntax, not when looking up or storing identities.
pub fn registration_email(raw: &str) -> Result<String, &'static str> {
    if raw.chars().any(char::is_control) {
        return Err(INVALID);
    }
    let normalized = normalize_email(raw)?;
    delivery_address(&normalized)?;
    Ok(normalized)
}

/// Conservative SMTP mailbox subset: ASCII dot-atom local part and a public
/// DNS-style domain (including IDNA). Quoted locals, address literals and
/// SMTPUTF8 local parts are not accepted until transport support is proven.
/// Return ASCII domain spelling without modifying the local part or the
/// stored account key. The caller must still resolve DNS and verify ownership.
pub fn delivery_address(address: &str) -> Result<String, &'static str> {
    if address.is_empty()
        || address.len() > 254
        || address.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(INVALID);
    }
    let (local, domain) = address.split_once('@').ok_or(INVALID)?;
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.bytes().all(valid_local_byte)
    {
        return Err(INVALID);
    }
    let ascii = idna::domain_to_ascii_strict(domain).map_err(|_| INVALID)?;
    let (host, suffix) = ascii.rsplit_once('.').ok_or(INVALID)?;
    if host.is_empty()
        || suffix.is_empty()
        || suffix.bytes().all(|b| b.is_ascii_digit())
        || ascii.len() > 253
        || ascii.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        || local.len() + 1 + ascii.len() > 254
    {
        return Err(INVALID);
    }
    Ok(format!("{local}@{ascii}"))
}

fn valid_local_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_public_mailboxes() {
        for address in [
            "",
            "abc",
            "a@b",
            "a@@example.com",
            "@example.com",
            "a@",
            "a@example..com",
            "a@.example.com",
            "a@example.com.",
            "a@-example.com",
            "a@example-.com",
            "a@exa_mple.com",
            "a@127.0.0.1",
            "a@[127.0.0.1]",
            ".a@example.com",
            "a.@example.com",
            "a..b@example.com",
            "a b@example.com",
            "a@example.com\n",
            "a\r@example.com",
            "a@example.com\t",
            "Name <a@example.com>",
            "\"a\"@example.com",
            "a@example.com:25",
            "а@example.com",
        ] {
            assert!(registration_email(address).is_err(), "{address:?}");
        }
    }

    #[test]
    fn allows_aliases_idna_and_common_mailboxes_without_rewriting_identity() {
        for address in [
            "reader@gmail.com",
            "reader+books@gmail.com",
            "a.b-c_d@sub.example.com",
            "o'connor@example.com",
            "a@xn--e1afmkfd.xn--p1ai",
            "a@пример.рф",
            "a@example.invalid",
        ] {
            assert_eq!(registration_email(address).unwrap(), address);
        }
        assert_eq!(
            registration_email("  Alice@Example.COM ").unwrap(),
            "alice@example.com"
        );
        assert_eq!(
            delivery_address("A+Books@пример.рф").unwrap(),
            "A+Books@xn--e1afmkfd.xn--p1ai"
        );
        assert_ne!(
            registration_email("a@пример.рф").unwrap(),
            registration_email("a@xn--e1afmkfd.xn--p1ai").unwrap()
        );
    }

    #[test]
    fn enforces_byte_and_label_boundaries() {
        let local = "a".repeat(64);
        let label = "b".repeat(63);
        assert!(delivery_address(&format!("{local}@{label}.com")).is_ok());
        assert!(delivery_address(&format!("{local}a@example.com")).is_err());
        assert!(delivery_address(&format!("a@{label}b.com")).is_err());
        let longest = format!("{local}@{label}.{label}.{}.com", "c".repeat(58));
        assert_eq!(longest.len(), 254);
        assert!(delivery_address(&longest).is_ok());
        assert!(delivery_address(&format!("{longest}m")).is_err());
    }
}
