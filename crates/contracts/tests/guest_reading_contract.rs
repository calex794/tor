use tor_contracts::guest_reading::{GuestFreeText, GuestPolicyError, GuestReadingPolicy};

#[test]
fn auth_read_default_open_and_explicit_prefix_boundaries() {
    for chapter in [1, 10, 11, 300, i32::MAX] {
        assert_eq!(
            GuestReadingPolicy::default().for_public_free(chapter),
            Ok(GuestFreeText::Full)
        );
    }
    let policy = GuestReadingPolicy::from_db("login_preview", 10, 20).unwrap();
    // Paid/held/absent numbers are never fed to this rule as eligible FREE.
    // Missing chapter 9 cannot shift the boundary and open chapter 11.
    for chapter in [1, 8, 10] {
        assert_eq!(policy.for_public_free(chapter), Ok(GuestFreeText::Full));
    }
    for chapter in [11, 300, i32::MAX] {
        assert_eq!(
            policy.for_public_free(chapter),
            Ok(GuestFreeText::Excerpt { percent: 20 })
        );
    }
}

#[test]
fn auth_read_zero_cutoff_percent_choices_and_policy_changes() {
    for percent in [10_i16, 20, 30, 50] {
        let policy = GuestReadingPolicy::from_db("login_preview", 0, percent).unwrap();
        assert_eq!(
            policy.for_public_free(1),
            Ok(GuestFreeText::Excerpt {
                percent: percent as u8,
            })
        );
    }
    for (n, expected) in [
        (10, GuestFreeText::Excerpt { percent: 20 }),
        (30, GuestFreeText::Full),
        (10, GuestFreeText::Excerpt { percent: 20 }),
    ] {
        let policy = GuestReadingPolicy::from_db("login_preview", n, 20).unwrap();
        assert_eq!(policy.for_public_free(20), Ok(expected));
    }
    let open = GuestReadingPolicy::from_db("open", 0, 10).unwrap();
    assert_eq!(open.for_public_free(300), Ok(GuestFreeText::Full));
}

#[test]
fn auth_read_invalid_persisted_facts_never_default_open() {
    assert_eq!(
        GuestReadingPolicy::from_db("unknown", 10, 20),
        Err(GuestPolicyError::UnknownMode)
    );
    assert_eq!(
        GuestReadingPolicy::from_db("open", -1, 20),
        Err(GuestPolicyError::InvalidOpenThrough)
    );
    for percent in [-1, 0, 15, 100, i16::MAX] {
        assert_eq!(
            GuestReadingPolicy::from_db("login_preview", 10, percent),
            Err(GuestPolicyError::InvalidPreviewPercent)
        );
    }
    for number in [i32::MIN, -1, 0] {
        assert_eq!(
            GuestReadingPolicy::default().for_public_free(number),
            Err(GuestPolicyError::InvalidChapterNumber)
        );
    }
}
