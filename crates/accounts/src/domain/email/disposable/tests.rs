use super::*;

#[test]
fn bundled_snapshot_filters_known_disposable_domains_without_provider_false_matches() {
    for domain in [
        "mailinator.com",
        "guerrillamail.com",
        "10minutemail.com",
        "sub.mailinator.com",
    ] {
        assert_eq!(
            classify(&format!("reader+alias@{domain}")).unwrap(),
            DisposableStatus::Listed
        );
    }
    for domain in [
        "gmail.com",
        "yandex.ru",
        "mail.ru",
        "outlook.com",
        "proton.me",
        "icloud.com",
    ] {
        assert_eq!(
            classify(&format!("reader@{domain}")).unwrap(),
            DisposableStatus::NotListed,
            "{domain}"
        );
    }
}

#[test]
fn matches_canonical_idna_labels_and_respects_exact_exceptions() {
    let rules = Rules::parse(
        "mailinator.com\nxn--e1afmkfd.xn--p1ai\n",
        "# Reviewed exception\nreal.mailinator.com\n",
    )
    .unwrap();
    for address in [
        "reader@MAILINATOR.COM",
        "reader@a.mailinator.com",
        "reader@пример.рф",
        "reader@xn--e1afmkfd.xn--p1ai",
        "reader@sub.real.mailinator.com",
    ] {
        assert_eq!(rules.classify(address).unwrap(), DisposableStatus::Listed);
    }
    assert_eq!(
        rules.classify("reader@real.mailinator.com").unwrap(),
        DisposableStatus::Exception
    );
    for address in [
        "reader@notmailinator.com",
        "reader@mailinator.com.example.org",
    ] {
        assert_eq!(
            rules.classify(address).unwrap(),
            DisposableStatus::NotListed
        );
    }
    assert!(rules.classify("reader@@mailinator.com").is_err());
}

#[test]
fn observe_is_default_and_does_not_block() {
    let mode = DisposableMode::default();
    assert_eq!(mode, DisposableMode::Observe);
    assert!(!mode.should_block(DisposableStatus::Listed));
    assert!(DisposableMode::Enforce.should_block(DisposableStatus::Listed));
    assert!(!DisposableMode::Enforce.should_block(DisposableStatus::Exception));
    assert!(!DisposableMode::Enforce.should_block(DisposableStatus::NotListed));
}

#[test]
fn malformed_empty_duplicate_and_unsorted_data_is_rejected() {
    for invalid in [
        "",
        "a.example\n\n",
        "a.example\na.example\n",
        "z.example\na.example\n",
        "a..example\n",
        "A.example\n",
        "com\n",
        "a.example.\n",
    ] {
        assert!(Rules::parse(invalid, "").is_err(), "{invalid:?}");
    }
    assert!(Rules::parse("a.example\n", "bad\n").is_err());
}
