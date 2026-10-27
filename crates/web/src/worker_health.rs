//! Classification is conservative: stale leases and retries are signals,
//! not proof a remote job process crashed. The RIL heartbeat is local only.
use tor_ingest::repositories::cover_jobs::CoverQueueCounts;
use crate::cover_worker::CoverRunnerSnapshot;
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
            warning: queue.is_some_and(|q| {
                q.overdue_pending > 0
                    || q.expired_lease > 0
                    || q.dead > 0
                    || q.repeated_attempts > 0
            }),
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
    if queue.is_some_and(|q| q.repeated_attempts > 0) {
        return Diagnostic {
            text: "У заданий RIL три и более попытки; требуется проверка причин повторов.",
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
    if queue.repeated_attempts > 0 {
        return Diagnostic {
            text: "Обложки: три и более попытки выполнения; нужно проверить исполнителя.",
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

/// Conservative local runner health. A live PostgreSQL lease is not evidence
/// that this or another process is making forward progress.
pub(crate) fn cover_runner(
    runner: Option<CoverRunnerSnapshot>,
    queue: Option<CoverQueueCounts>,
) -> Diagnostic {
    let base = covers(queue);
    let Some(runner) = runner else {
        return base;
    };
    if runner.stopped || runner.timeouts > 0 {
        return Diagnostic {
            text: "Локальный исполнитель обложек остановлен после превышения времени операции; требуется проверка.",
            warning: true,
        };
    }
    if runner.running_for_s.is_some_and(|seconds| seconds >= 95)
        || runner.since_completion_s.is_some_and(|seconds| seconds >= 95)
        || (runner.cycles == 0 && runner.since_start_s.is_some_and(|seconds| seconds >= 95))
    {
        return Diagnostic {
            text: "Исполнитель обложек не завершал цикл более 95 секунд: возможная остановка.",
            warning: true,
        };
    }
    if runner.consecutive_errors >= 3 {
        return Diagnostic {
            text: "Исполнитель обложек завершил три или более операций подряд с ошибками.",
            warning: true,
        };
    }
    if base.warning {
        return base;
    }
    if runner.cycles == 0 {
        Diagnostic {
            text: "Локальный исполнитель обложек запущен; первый цикл ещё не завершён.",
            warning: false,
        }
    } else {
        Diagnostic {
            text: "Локальный исполнитель обложек завершает циклы; состояние других процессов неизвестно.",
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
        queue.overdue_pending = 0;
        queue.dead = 1;
        assert!(ril_pump(None, Some(queue)).warning);
        queue.dead = 0;
        queue.repeated_attempts = 1;
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
        queue.repeated_attempts = 2;
        assert!(covers(Some(queue)).warning);
        queue.repeated_attempts = 0;
        queue.pending = 1;
        let info = covers(Some(queue));
        assert!(!info.warning);
        assert!(info.text.contains("не подтверждён"));
    }

    #[test]
    fn cover_runner_timeout_and_stale_heartbeat_are_warnings_even_when_queue_empty() {
        let mut runner = CoverRunnerSnapshot {
            cycles: 2, since_completion_s: Some(96), ..CoverRunnerSnapshot::default()
        };
        assert!(cover_runner(Some(runner), Some(cover_queue())).warning);
        runner.since_completion_s = Some(1);
        runner.timeouts = 1;
        runner.stopped = true;
        assert!(cover_runner(Some(runner), Some(cover_queue())).warning);
        runner.stopped = false;
        runner.timeouts = 0;
        runner.consecutive_errors = 3;
        assert!(cover_runner(Some(runner), Some(cover_queue())).warning);
    }

    #[test]
    fn cover_queue_and_runtime_must_not_invent_success() {
        assert!(cover_runner(None, None).text.contains("не подтверждена"));
        let first = CoverRunnerSnapshot {
            since_start_s: Some(1), ..CoverRunnerSnapshot::default()
        };
        assert!(!cover_runner(Some(first), Some(cover_queue())).warning);
        assert!(cover_runner(Some(first), Some(cover_queue())).text.contains("первый цикл"));
        let mut queue = cover_queue();
        queue.overdue_pending = 1;
        assert!(cover_runner(Some(first), Some(queue)).warning);
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
