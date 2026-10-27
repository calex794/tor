//! Privacy-safe, fixed-cardinality outcomes of completed POST /login attempts.
//! A process restart resets these counters; they are not a persistent audit log.
use std::{
    fmt::Write,
    sync::atomic::{AtomicU64, Ordering},
};

/// Only fixed outcomes declared in source code become Prometheus labels.
/// No email, network address, account ID, path, or request payload is accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum LoginOutcome {
    Success,
    InvalidCredentials,
    InvalidForm,
    RateLimited,
    RateLimiterUnavailable,
    PasswordPoolBusy,
    PasswordWorkerUnavailable,
    LookupUnavailable,
    AttemptResetUnavailable,
    SessionUnavailable,
}

impl LoginOutcome {
    pub const ALL: [Self; 10] = [
        Self::Success,
        Self::InvalidCredentials,
        Self::InvalidForm,
        Self::RateLimited,
        Self::RateLimiterUnavailable,
        Self::PasswordPoolBusy,
        Self::PasswordWorkerUnavailable,
        Self::LookupUnavailable,
        Self::AttemptResetUnavailable,
        Self::SessionUnavailable,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::InvalidCredentials => "invalid_credentials",
            Self::InvalidForm => "invalid_form",
            Self::RateLimited => "rate_limited",
            Self::RateLimiterUnavailable => "rate_limiter_unavailable",
            Self::PasswordPoolBusy => "password_pool_busy",
            Self::PasswordWorkerUnavailable => "password_worker_unavailable",
            Self::LookupUnavailable => "lookup_unavailable",
            Self::AttemptResetUnavailable => "attempt_reset_unavailable",
            Self::SessionUnavailable => "session_unavailable",
        }
    }

    pub const fn operator_label(self) -> &'static str {
        match self {
            Self::Success => "Успешный вход",
            Self::InvalidCredentials => "Неверные учётные данные",
            Self::InvalidForm => "Ошибка формы или проверок источника",
            Self::RateLimited => "Превышено число попыток",
            Self::RateLimiterUnavailable => "Сбой хранилища ограничений",
            Self::PasswordPoolBusy => "Очередь проверки пароля занята",
            Self::PasswordWorkerUnavailable => "Сбой проверки пароля",
            Self::LookupUnavailable => "Сбой поиска учётной записи",
            Self::AttemptResetUnavailable => "Сбой сброса ограничений",
            Self::SessionUnavailable => "Сбой создания сеанса",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoginSnapshot {
    counts: [u64; LoginOutcome::ALL.len()],
}

impl LoginSnapshot {
    pub fn count(self, outcome: LoginOutcome) -> u64 {
        self.counts[outcome as usize]
    }

    pub fn total(self) -> u64 {
        self.counts
            .into_iter()
            .fold(0_u64, u64::saturating_add)
    }
}

/// Atomic counters keep authentication independent of metrics I/O or SQLite.
pub struct LoginMetrics {
    counters: [AtomicU64; LoginOutcome::ALL.len()],
}

impl Default for LoginMetrics {
    fn default() -> Self {
        Self {
            counters: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl LoginMetrics {
    pub fn record(&self, outcome: LoginOutcome) {
        // Never wrap a Prometheus counter, even if the process lives for years.
        let _ = self.counters[outcome as usize].fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_add(1)),
        );
    }

    pub fn snapshot(&self) -> LoginSnapshot {
        LoginSnapshot {
            counts: std::array::from_fn(|index| self.counters[index].load(Ordering::Relaxed)),
        }
    }

    pub fn prometheus(&self) -> String {
        let snapshot = self.snapshot();
        let mut output = String::from(
            "# HELP tor_auth_login_outcomes_total Completed login outcomes (process-local, no identifiers).\n\
             # TYPE tor_auth_login_outcomes_total counter\n",
        );
        for outcome in LoginOutcome::ALL {
            writeln!(
                output,
                "tor_auth_login_outcomes_total{{outcome=\"{}\"}} {}",
                outcome.label(),
                snapshot.count(outcome)
            )
            .expect("write to String");
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_cumulative_and_outcomes_remain_independent() {
        let metrics = LoginMetrics::default();
        for _ in 0..200 {
            metrics.record(LoginOutcome::LookupUnavailable);
        }
        metrics.record(LoginOutcome::RateLimited);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.count(LoginOutcome::LookupUnavailable), 200);
        assert_eq!(snapshot.count(LoginOutcome::RateLimited), 1);
        assert_eq!(snapshot.count(LoginOutcome::Success), 0);
        assert_eq!(snapshot.total(), 201);
        assert!(metrics.prometheus().contains(
            "tor_auth_login_outcomes_total{outcome=\"lookup_unavailable\"} 200"
        ));
    }

    #[test]
    fn exported_labels_are_only_bounded_source_owned_values() {
        let metrics = LoginMetrics::default();
        for outcome in LoginOutcome::ALL {
            metrics.record(outcome);
        }
        let lines = metrics.prometheus();
        assert_eq!(
            lines.lines().filter(|line| line.starts_with("tor_auth_login_outcomes_total{")).count(),
            10
        );
        for forbidden in ["email", "user_id", "request_id", "remote_ip", "cookie", "sql_text"] {
            assert!(!lines.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn totals_saturate_instead_of_wrapping() {
        let metrics = LoginMetrics::default();
        metrics.counters[LoginOutcome::Success as usize]
            .store(u64::MAX, Ordering::Relaxed);
        metrics.record(LoginOutcome::Success);
        assert_eq!(metrics.snapshot().count(LoginOutcome::Success), u64::MAX);
    }
}
