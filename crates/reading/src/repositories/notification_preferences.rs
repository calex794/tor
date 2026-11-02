//! Reader-owned future delivery preference; never creates an actual delivery.
#![forbid(unsafe_code)]
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    Disabled,
    Immediate,
    DailyDigest,
}

impl DeliveryMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "immediate" => Some(Self::Immediate),
            "daily_digest" => Some(Self::DailyDigest),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Immediate => "immediate",
            Self::DailyDigest => "daily_digest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceError {
    Invalid,
    Unavailable,
}

#[derive(Clone)]
pub struct PgNotificationPreferences {
    pool: PgPool,
}

impl PgNotificationPreferences {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// An absent row never implicitly opts a reader into external messages.
    pub async fn delivery_preference(&self, actor: i64) -> Result<DeliveryMode, PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT delivery_mode FROM reader_notification_delivery_preferences WHERE user_id=$1",
        )
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| PreferenceError::Unavailable)?;
        match raw {
            None => Ok(DeliveryMode::Disabled),
            Some(mode) => DeliveryMode::parse(&mode).ok_or(PreferenceError::Unavailable),
        }
    }

    /// One scoped row per reader. Does not enqueue messages or alter book follows.
    pub async fn set_delivery_preference(
        &self,
        actor: i64,
        mode: DeliveryMode,
    ) -> Result<(), PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let result = sqlx::query(
            "INSERT INTO reader_notification_delivery_preferences(user_id,delivery_mode)
             SELECT id,$2 FROM users WHERE id=$1
             ON CONFLICT(user_id) DO UPDATE
             SET delivery_mode=EXCLUDED.delivery_mode,updated_at=now()",
        )
        .bind(actor)
        .bind(mode.as_str())
        .execute(&self.pool)
        .await
        .map_err(|_| PreferenceError::Unavailable)?;
        if result.rows_affected() == 0 {
            return Err(PreferenceError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DeliveryMode;

    #[test]
    fn delivery_modes_are_explicit_and_closed() {
        for mode in [
            DeliveryMode::Disabled,
            DeliveryMode::Immediate,
            DeliveryMode::DailyDigest,
        ] {
            assert_eq!(DeliveryMode::parse(mode.as_str()), Some(mode));
        }
        for invalid in ["", "instant", "smart_time", "DISABLED", "email", "all"] {
            assert_eq!(DeliveryMode::parse(invalid), None);
        }
    }
}
