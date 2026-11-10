//! The notification delivery mode is a reader preference, not a SQL record.
//! It does not authorize sending messages or changing delivery channels.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    Disabled,
    Immediate,
    SmartTime,
    DailyDigest,
}

impl DeliveryMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "immediate" => Some(Self::Immediate),
            "smart_time" => Some(Self::SmartTime),
            "daily_digest" => Some(Self::DailyDigest),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Immediate => "immediate",
            Self::SmartTime => "smart_time",
            Self::DailyDigest => "daily_digest",
        }
    }
}
