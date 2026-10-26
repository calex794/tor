//! Validated, bounded keyset cursor for public chapter lists (C-03 / S005).
#![forbid(unsafe_code)]

pub const DEFAULT_PAGE_SIZE: u16 = 100;
pub const MAX_PAGE_SIZE: u16 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TocCursor {
    pub chapter_no: i32,
    pub chapter_id: i64,
}

impl TocCursor {
    /// The cursor is a public position, never a permission or a database query.
    pub fn parse(input: &str) -> Option<Self> {
        if input.len() > 40 {
            return None;
        }
        let (number, id) = input.split_once('.')?;
        if number.is_empty()
            || id.is_empty()
            || !number.bytes().all(|byte| byte.is_ascii_digit())
            || !id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let chapter_no = number.parse::<i32>().ok()?;
        let chapter_id = id.parse::<i64>().ok()?;
        if chapter_no <= 0 || chapter_id <= 0 {
            return None;
        }
        Some(Self {
            chapter_no,
            chapter_id,
        })
    }

    pub fn encode(chapter_no: i32, chapter_id: u64) -> String {
        format!("{chapter_no}.{chapter_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_and_rejection() {
        assert_eq!(
            TocCursor::parse(&TocCursor::encode(501, 123_456)),
            Some(TocCursor {
                chapter_no: 501,
                chapter_id: 123_456
            })
        );
        for invalid in [
            "",
            "0.1",
            "1.0",
            "-1.2",
            "1.-2",
            "1.2.3",
            "1",
            "+1.2",
            "1.999999999999999999999",
            "2147483648.1",
            "01.2 ",
            "1.2%20",
        ] {
            assert!(TocCursor::parse(invalid).is_none(), "{invalid}");
        }
    }
}
