use serde::{de::DeserializeOwned, Serialize};
use std::{fmt::Debug, str::FromStr};
use tor_contracts::{
    BookId, ChapterId, ChapterVersionId, IdempotencyKey, InvalidId, PositiveId, PurchaseId, UserId,
};

fn check_identifier<T>()
where
    T: Serialize + DeserializeOwned + Debug + Eq + FromStr<Err = InvalidId>,
{
    for raw in [
        "1",
        "9007199254740993",
        "1783709600694950942",
        "9223372036854775807",
    ] {
        let value: T = raw.parse().unwrap();
        let json = format!("\"{raw}\"");
        assert_eq!(serde_json::to_string(&value).unwrap(), json);
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), value);
        // Even an exactly representable JSON number is not a v1 identifier.
        assert!(serde_json::from_str::<T>(raw).is_err());
    }
    for raw in [
        "",
        "0",
        "-1",
        "+1",
        " 1",
        "1 ",
        "1.0",
        "1e3",
        "１２",
        "١",
        "1\n",
        "9223372036854775808",
        "18446744073709551615",
        "00000000000000000001",
    ] {
        assert!(raw.parse::<T>().is_err(), "accepted {raw:?}");
        assert!(serde_json::from_value::<T>(serde_json::json!(raw)).is_err());
    }
    for json in ["null", "true", "{}", "[]", "1.0", "1e3", "-1"] {
        assert!(serde_json::from_str::<T>(json).is_err(), "accepted {json}");
    }
    // The existing progress adapter permits leading zeroes; output is canonical.
    let legacy: T = "001".parse().unwrap();
    assert_eq!(serde_json::to_string(&legacy).unwrap(), "\"1\"");
}

macro_rules! identifier_test {
    ($test:ident, $id:ty) => {
        #[test]
        fn $test() {
            check_identifier::<$id>();
            for value in [i64::MIN, -1, 0] {
                assert_eq!(<$id>::try_from(value), Err(InvalidId));
            }
            for value in [1_i64, i64::MAX] {
                assert_eq!(<$id>::try_from(value).unwrap().get(), value);
            }
            assert_eq!(<$id>::try_from(0_u64), Err(InvalidId));
            assert_eq!(<$id>::try_from(i64::MAX as u64).unwrap().get(), i64::MAX);
            assert_eq!(<$id>::try_from(i64::MAX as u64 + 1), Err(InvalidId));
            assert_eq!(<$id>::try_from(u64::MAX), Err(InvalidId));
        }
    };
}

identifier_test!(positive_id_wire, PositiveId);
identifier_test!(user_id_wire, UserId);
identifier_test!(book_id_wire, BookId);
identifier_test!(chapter_id_wire, ChapterId);
identifier_test!(chapter_version_id_wire, ChapterVersionId);
identifier_test!(purchase_id_wire, PurchaseId);

#[test]
fn idempotency_key_preserves_the_command_table_grammar() {
    for raw in [
        "a".repeat(43),
        "b".repeat(43),
        format!("{}-_A09", "z".repeat(38)),
    ] {
        let key: IdempotencyKey = raw.parse().unwrap();
        assert_eq!(key.as_str(), raw);
        assert_eq!(String::from(key.clone()), raw);
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(serde_json::from_str::<IdempotencyKey>(&json).unwrap(), key);
        assert_eq!(serde_json::from_str::<String>(&json).unwrap(), raw);
    }
    for raw in [
        String::new(),
        "a".repeat(42),
        "a".repeat(44),
        format!("{}=", "a".repeat(42)),
        format!("{}+", "a".repeat(42)),
        format!("{}/", "a".repeat(42)),
        format!("{} ", "a".repeat(42)),
        format!("{}\n", "a".repeat(42)),
        format!("{}é", "a".repeat(41)),
    ] {
        assert!(raw.parse::<IdempotencyKey>().is_err(), "accepted {raw:?}");
        assert!(serde_json::from_value::<IdempotencyKey>(serde_json::json!(raw)).is_err());
    }
    for json in ["null", "43", "{}", "[]", "true"] {
        assert!(serde_json::from_str::<IdempotencyKey>(json).is_err());
    }
}
