//! AUTH-READ: book policy applied only after B/C verify current FREE access.
//!
//! No session, storage capability, price, publication or SEO authority here.
//! Both reader and SEO fact producers must use the same chapter-number rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestPolicyError {
    UnknownMode,
    InvalidOpenThrough,
    InvalidPreviewPercent,
    InvalidChapterNumber,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestFreeText {
    Full,
    Excerpt { percent: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestReadingPolicy {
    mode: GuestReadMode,
    open_through: i32,
    preview_percent: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuestReadMode {
    Open,
    LoginPreview,
}

impl GuestReadingPolicy {
    /// Decode trusted persisted facts. Unknown policy is an error, never open.
    pub fn from_db(
        mode: &str,
        open_through: i32,
        preview_percent: i16,
    ) -> Result<Self, GuestPolicyError> {
        let mode = match mode {
            "open" => GuestReadMode::Open,
            "login_preview" => GuestReadMode::LoginPreview,
            _ => return Err(GuestPolicyError::UnknownMode),
        };
        if open_through < 0 {
            return Err(GuestPolicyError::InvalidOpenThrough);
        }
        let preview_percent = match preview_percent {
            10 | 20 | 30 | 50 => preview_percent as u8,
            _ => return Err(GuestPolicyError::InvalidPreviewPercent),
        };
        Ok(Self {
            mode,
            open_through,
            preview_percent,
        })
    }

    pub fn mode(self) -> &'static str {
        match self.mode {
            GuestReadMode::Open => "open",
            GuestReadMode::LoginPreview => "login_preview",
        }
    }

    pub fn open_through(self) -> i32 {
        self.open_through
    }

    pub fn preview_percent(self) -> i16 {
        i16::from(self.preview_percent)
    }

    /// The caller must first verify public, current, released, age-permitted
    /// FREE access. This describes guest text extent, not an entitlement.
    /// Paid/held/deleted/draft content cannot be passed as eligible FREE.
    pub fn for_public_free(self, chapter_no: i32) -> Result<GuestFreeText, GuestPolicyError> {
        if chapter_no <= 0 {
            return Err(GuestPolicyError::InvalidChapterNumber);
        }
        if self.mode == GuestReadMode::Open || chapter_no <= self.open_through {
            Ok(GuestFreeText::Full)
        } else {
            Ok(GuestFreeText::Excerpt {
                percent: self.preview_percent,
            })
        }
    }
}

impl Default for GuestReadingPolicy {
    fn default() -> Self {
        Self {
            mode: GuestReadMode::Open,
            open_through: 0,
            preview_percent: 20,
        }
    }
}
