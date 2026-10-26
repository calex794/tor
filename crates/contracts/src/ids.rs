use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, num::NonZeroI64, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidId;

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected a positive decimal BIGINT identifier")
    }
}

impl Error for InvalidId {}

fn parse(raw: &str) -> Result<i64, InvalidId> {
    // Preserve the existing progress parser's acceptance of leading zeroes,
    // while bounding work and rejecting signs, whitespace and non-ASCII digits.
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InvalidId);
    }
    raw.parse().map_err(|_| InvalidId)
}

macro_rules! identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(NonZeroI64);

        impl $name {
            pub fn get(self) -> i64 {
                self.0.get()
            }
        }

        impl TryFrom<i64> for $name {
            type Error = InvalidId;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                NonZeroI64::new(value)
                    .filter(|id| id.get() > 0)
                    .map(Self)
                    .ok_or(InvalidId)
            }
        }

        impl TryFrom<u64> for $name {
            type Error = InvalidId;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                Self::try_from(i64::try_from(value).map_err(|_| InvalidId)?)
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                Self::try_from(parse(raw)?)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidId;

            fn try_from(raw: String) -> Result<Self, Self::Error> {
                raw.parse()
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.to_string()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

identifier!(
    PositiveId,
    "A positive BIGINT at a legacy, untyped boundary."
);
identifier!(
    UserId,
    "An account identifier; never a client-supplied permission."
);
identifier!(
    BookId,
    "A book identifier, distinct from a chapter identifier."
);
identifier!(
    ChapterId,
    "A chapter identifier, distinct from its immutable edition."
);
identifier!(ChapterVersionId, "An immutable chapter edition identifier.");
identifier!(
    PurchaseId,
    "A purchase identifier; not proof of entitlement."
);
