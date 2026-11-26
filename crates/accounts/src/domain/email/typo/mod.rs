//! MAIL-01e: advisory domain typo hints, not email validity or identity rules.
use super::delivery_address;

/// Return a known provider domain for an explicitly reviewed typo only.
/// This deliberately does not do fuzzy matching across arbitrary domains.
/// No address mutation, DNS lookup, automatic rejection or mail submission.
pub fn suggested_domain(address: &str) -> Option<&'static str> {
    let transport = delivery_address(address).ok()?;
    let (_, domain) = transport.rsplit_once('@')?;
    match domain.to_ascii_lowercase().as_str() {
        "gmial.com" | "gamil.com" | "gmal.com" | "gmail.con" | "gmail.cmo" => Some("gmail.com"),
        "yadnex.ru" | "yandx.ru" | "yandex.ur" | "yandex.r" => Some("yandex.ru"),
        "mial.ru" | "maill.ru" | "mail.ur" => Some("mail.ru"),
        "outlok.com" | "outllook.com" | "outlook.con" => Some("outlook.com"),
        "hotmial.com" | "hotmal.com" | "hotmail.con" => Some("hotmail.com"),
        "icoud.com" | "iclod.com" | "icloud.con" => Some("icloud.com"),
        "protn.me" => Some("proton.me"),
        "protonmail.con" => Some("protonmail.com"),
        "ramlber.ru" | "rambler.ur" => Some("rambler.ru"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
