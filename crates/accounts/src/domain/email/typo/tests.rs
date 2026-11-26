use super::*;

#[test]
fn reviewed_typos_offer_only_a_replacement_domain() {
    let cases = [
        ("reader+books@gmial.com", "gmail.com"),
        ("Reader@GMAIL.CON", "gmail.com"),
        ("reader@yadnex.ru", "yandex.ru"),
        ("reader@yandex.ur", "yandex.ru"),
        ("reader@mial.ru", "mail.ru"),
        ("reader@outlok.com", "outlook.com"),
        ("reader@hotmial.com", "hotmail.com"),
        ("reader@icoud.com", "icloud.com"),
        ("reader@protn.me", "proton.me"),
        ("reader@protonmail.con", "protonmail.com"),
        ("reader@ramlber.ru", "rambler.ru"),
    ];
    for (address, expected) in cases {
        assert_eq!(suggested_domain(address), Some(expected), "{address}");
        let transport = delivery_address(address).unwrap();
        assert_eq!(transport.split('@').next(), address.split('@').next());
    }
}

#[test]
fn valid_unknown_and_international_domains_are_not_guessed() {
    for address in [
        "reader@gmail.com",
        "reader@googlemail.com",
        "reader@yandex.ru",
        "reader@ya.ru",
        "reader@yandex.com",
        "reader@mail.ru",
        "reader@inbox.ru",
        "reader@list.ru",
        "reader@bk.ru",
        "reader@outlook.com",
        "reader@hotmail.com",
        "reader@icloud.com",
        "reader@proton.me",
        "reader@protonmail.com",
        "reader@rambler.ru",
        "reader@example.org",
        "reader@пример.рф",
        "reader@xn--e1afmkfd.xn--p1ai",
        "reader@sub.gmial.com",
        "reader@gmial.com.example.org",
        "reader@company-gmial.com",
        "gmial.com@example.org",
        "reader@gmx.com",
        "reader@gmail.ru",
        "reader@yandex.net",
        "reader@outlook.de",
    ] {
        assert_eq!(suggested_domain(address), None, "{address}");
    }
}

#[test]
fn invalid_mailboxes_do_not_produce_hints() {
    for address in [
        "gmial.com",
        "reader@@gmial.com",
        "reader..name@gmial.com",
        "reader@gmial..com",
        "reader@gmial.com\n",
        "reader @gmial.com",
    ] {
        assert_eq!(suggested_domain(address), None, "{address}");
    }
}

#[test]
fn a_hint_never_rewrites_registration_identity() {
    let address = "Reader+Books@GMIAL.COM";
    assert_eq!(suggested_domain(address), Some("gmail.com"));
    assert_eq!(
        super::super::registration_email(address).unwrap(),
        "reader+books@gmial.com"
    );
}
