//! Classification is conservative: stale leases and retries are signals,
//! not proof a remote job process crashed. The RIL heartbeat is local only.
use tor_ingest::repositories::cover_jobs::CoverQueueCounts;
use tor_traffic_behavior::{repositories::RilQueueCounts, services::PgPumpSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub text: &'static str,
    pub warning: bool,
}

pub(crate) fn ril_pump(pump: Option<PgPumpSnapshot>, queue: Option<RilQueueCounts>) -> Diagnostic {
    let Some(pump) = pump else {
        return Diagnostic {
            text: "В этом процессе сборщик RIL не запускался; другие процессы не проверены.",
            warning: queue.is_some_and(|q| q.overdue_pending > 0 || q.expired_lease > 0),
        };
    };
    if pump.running_for_s.is_some_and(|s| s >= 90)
        || pump.age_since_last_completion_s.is_some_and(|s| s >= 90)
        || (pump.cycles == 0 && pump.age_since_start_s.is_some_and(|s| s >= 90))
    {
        return Diagnostic {
            text: "Не было завершённого цикла более 90 секунд: возможное зависание или остановка.",
            warning: true,
        };
    }
    if pump.consecutive_failures >= 3 {
        return Diagnostic {
            text: "Три и более последовательных цикла RIL завершились с техническими ошибками.",
            warning: true,
        };
    }
    if queue.is_some_and(|q| q.expired_lease > 0 || q.overdue_pending > 0) {
        return Diagnostic {
            text: "В очереди RIL найдены просроченные задания; требуется проверка обработки.",
            warning: true,
        };
    }
    if queue.is_some_and(|q| q.dead > 0) {
        return Diagnostic {
            text:
                "Есть задания RIL с окончательной ошибкой; цикл сборщика может продолжать работать.",
            warning: true,
        };
    }
    if pump.cycles == 0 {
        Diagnostic {
            text: "Исполнитель RIL создан; первый цикл ещё не завершён.",
            warning: false,
        }
    } else {
        Diagnostic {
            text: "Недавний цикл RIL завершён; это не подтверждает обработку всех заданий.",
            warning: false,
        }
    }
}

pub(crate) fn covers(queue: Option<CoverQueueCounts>) -> Diagnostic {
    let Some(queue) = queue else {
        return Diagnostic {
            text: "Нет данных о заданиях; работа отдельного исполнителя не подтверждена.",
            warning: false,
        };
    };
    if queue.expired_lease > 0 || queue.overdue_pending > 0 {
        return Diagnostic {
            text: "Обложки: обнаружена просроченная очередь или истёкшая аренда задания.",
            warning: true,
        };
    }
    if queue.dead > 0 {
        return Diagnostic {
            text: "Обложки: имеются задания с окончательной ошибкой.",
            warning: true,
        };
    }
    if queue.pending > 0 || queue.running > 0 || queue.ready > 0 {
        Diagnostic {
            text: "Есть незавершённые задания; отдельный исполнитель не подтверждён.",
            warning: false,
        }
    } else {
        Diagnostic {
            text: "Нет незавершённых заданий. Это не доказательство работы исполнителя.",
            warning: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ril_queue() -> RilQueueCounts {
        RilQueueCounts {
            pending: 0,
            running: 0,
            dead: 0,
            overdue_pending: 0,
            expired_lease: 0,
            repeated_attempts: 0,
        }
    }

    fn cover_queue() -> CoverQueueCounts {
        CoverQueueCounts {
            pending: 0,
            running: 0,
            ready: 0,
            dead: 0,
            overdue_pending: 0,
            expired_lease: 0,
            repeated_attempts: 0,
        }
    }

    #[test]
    fn stale_idle_pump_is_not_healthy_even_without_backlog() {
        let pump = PgPumpSnapshot {
            cycles: 1,
            age_since_last_completion_s: Some(91),
            ..PgPumpSnapshot::default()
        };
        assert!(ril_pump(Some(pump), Some(ril_queue())).warning);
        let running = PgPumpSnapshot {
            active: true,
            running_for_s: Some(91),
            ..PgPumpSnapshot::default()
        };
        assert!(ril_pump(Some(running), Some(ril_queue())).warning);
        let never_started = PgPumpSnapshot {
            age_since_start_s: Some(100),
            ..PgPumpSnapshot::default()
        };
        assert!(ril_pump(Some(never_started), Some(ril_queue())).warning);
    }

    #[test]
    fn repeated_worker_failures_are_reported_without_user_ids() {
        let pump = PgPumpSnapshot {
            cycles: 8,
            consecutive_failures: 3,
            age_since_last_completion_s: Some(1),
            ..PgPumpSnapshot::default()
        };
        let text = ril_pump(Some(pump), Some(ril_queue()));
        assert!(text.warning);
        assert!(text.text.contains("последовательных"));
        assert!(!text.text.contains("user_id"));
    }

    #[test]
    fn no_process_is_unknown_and_queue_is_not_proof_of_heartbeat() {
        assert!(!ril_pump(None, None).warning);
        let mut queue = ril_queue();
        queue.overdue_pending = 1;
        assert!(ril_pump(None, Some(queue)).warning);
        assert!(!covers(Some(cover_queue())).warning);
        assert!(covers(None).text.contains("не подтверждена"));
    }

    #[test]
    fn overdue_and_expired_cover_jobs_trigger_attention_without_false_live_worker() {
        let mut queue = cover_queue();
        queue.expired_lease = 1;
        assert!(covers(Some(queue)).warning);
        queue.expired_lease = 0;
        queue.overdue_pending = 2;
        assert!(covers(Some(queue)).warning);
        queue.overdue_pending = 0;
        queue.dead = 1;
        assert!(covers(Some(queue)).warning);
        queue.dead = 0;
        queue.pending = 1;
        let info = covers(Some(queue));
        assert!(!info.warning);
        assert!(info.text.contains("не подтверждён"));
    }

    #[test]
    fn recent_healthy_cycle_and_empty_queue_are_not_flagged() {
        let pump = PgPumpSnapshot {
            cycles: 20,
            age_since_start_s: Some(300),
            age_since_last_completion_s: Some(5),
            ..PgPumpSnapshot::default()
        };
        assert!(!ril_pump(Some(pump), Some(ril_queue())).warning);
    }
}
