use super::{Evaluation, Reason, Status};

pub const COOLDOWN_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Episode {
    pub sequence: i64,
    pub reason: Option<Reason>,
    pub opened_ms: Option<i64>,
    pub opening_emitted: bool,
    pub last_notice_ms: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Opened,
    Recovered,
}
impl EventKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Recovered => "recovered",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub episode: Episode,
    pub event: Option<EventKind>,
}

/// Called only after the repository has fenced the writer and source scope.
/// Unknown and disabled snapshots preserve an open episode. A different active
/// reason also preserves it: a new failure is not proof of recovery.
pub fn transition(mut episode: Episode, result: Evaluation, now_ms: i64) -> Transition {
    let event = match result.status {
        Status::Active if episode.reason.is_none() => {
            episode.sequence = episode.sequence.saturating_add(1);
            episode.reason = Some(result.reason);
            episode.opened_ms = Some(now_ms);
            episode.opening_emitted = episode
                .last_notice_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= COOLDOWN_MS);
            if episode.opening_emitted {
                episode.last_notice_ms = Some(now_ms);
                Some(EventKind::Opened)
            } else {
                None
            }
        }
        Status::Healthy if episode.reason.is_some() => {
            let event = episode.opening_emitted.then_some(EventKind::Recovered);
            episode.reason = None;
            episode.opened_ms = None;
            episode.opening_emitted = false;
            event
        }
        _ => None,
    };
    Transition { episode, event }
}
