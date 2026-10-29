//! Server-derived effective account level. Values cannot be set by a
//! browser request; adult opt-in and pro_adult grants are separate A facts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectiveUserTier {
    Basic,
    Adult,
    ProAdult,
}

impl EffectiveUserTier {
    pub fn as_persisted(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Adult => "adult",
            Self::ProAdult => "pro_adult",
        }
    }
}
