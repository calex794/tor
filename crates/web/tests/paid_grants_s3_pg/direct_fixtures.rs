//! Real private-S3 regression assertions for exact direct paid V1 evidence.
use super::*;

pub(super) fn assert_not_issued(url: &str, purchase_id: i64) {
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        0,
        "checkout itself cannot record first protected issuance"
    );
}

pub(super) fn assert_first_issued(
    url: &str,
    purchase_id: i64,
    chapter: i64,
    v1: i64,
    purchase_two_id: i64,
) {
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT chapter_id || ':' || version_id || ':' || object_kind
                 FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        format!("{chapter}:{v1}:chapter"),
        "real paid MinIO response must mark the exact direct purchased V1"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_two_id}"
            ),
        ),
        0,
        "opening one purchased chapter cannot mark a different purchase"
    );
}
