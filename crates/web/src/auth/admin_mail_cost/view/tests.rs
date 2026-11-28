use super::*;
use tor_mail::domain::cost::{CostLimits, CostSnapshot, MonthUsage};

#[test]
fn shows_estimate_and_stop_without_activation_or_secret_fields() {
    let view = CostAdminView {
        revision: 7,
        snapshot: CostSnapshot {
            limits: CostLimits {
                enabled: false,
                warning_1: 10000,
                warning_2: 30000,
                news_pause: 40000,
                monthly_limit: 50000,
            },
            usage: Some(MonthUsage {
                month_start: "2026-11-01".to_owned(),
                free_allowance: 2000,
                baseline_kopecks: 0,
                rate_per_thousand: 8032,
                reserved_attempts: 5000,
            }),
            boundary_pause: false,
        },
    };
    let html = body(&view, "csrf<fixture>");
    assert!(html.contains("240.96 ₽"));
    assert!(html.contains("259.04 ₽"));
    assert!(html.contains("Первый порог расходов"));
    assert!(html.contains("Отправка остановлена"));
    assert!(html.contains("csrf&lt;fixture&gt;"));
    assert!(html.contains("name=\"revision\" value=\"7\""));
    assert!(!html.contains("name=\"enabled\""));
    assert!(!html.contains("type=\"password\""));
    assert!(html.contains("/odmen/system-mail/reconcile"));
    assert!(html.contains("name=\"free_allowance\" min=\"0\" max=\"2000\" value=\"2000\""));
    let mut unavailable = view;
    unavailable.snapshot.usage = None;
    let html = body(&unavailable, "");
    assert!(html.contains("Месяц не сверён"));
    assert!(html.contains("name=\"free_allowance\" min=\"0\" max=\"2000\" value=\"0\""));
    unavailable.snapshot.limits.enabled = true;
    assert!(!body(&unavailable, "").contains("/odmen/system-mail/reconcile"));
}
