//! Administrative aggregate-only presentation, never participant identities.
use crate::pages::escape_html;
use std::fmt::Write;
use tor_recommendations::{
    repositories::surprises::experiment::ExperimentReport,
    services::surprise_experiment::PRIVATE_MIN,
};

fn decision(code: &str) -> &str {
    match code {
        "collecting" => "Сбор выборки; вывод об эффекте запрещён",
        "observing" => "Наблюдение продолжается; итог ещё не подводится",
        "operator_stopped" => "Остановлен администратором; причинный эффект не установлен",
        "capture_failure" => "Остановлен: потеря измерений",
        "monitor_gap" => "Остановлен: пропуск проверки безопасности",
        "assignment_mismatch" => "Остановлен: нарушено распределение групп",
        "invalid_counts" => "Остановлен: некорректные счётчики",
        "incomplete_observation" => "Неполное окно наблюдения; результат недостоверен",
        "guardrail_harm" => "Остановлен: ухудшение защитного показателя",
        "insufficient_sample" => "Недостаточная выборка; эффект не доказан",
        "baseline_imbalance" => "Исходные группы несопоставимы; эффект не доказан",
        "guardrails_inconclusive" => "Основная метрика выросла, безопасность чтения не подтверждена",
        "gentle_supported" => "Положительный результат подтверждён только для мягкой группы",
        "standard_supported" => "Положительный результат подтверждён только для обычной группы",
        "both_supported" => "Положительный результат подтверждён для обеих групп",
        "no_demonstrated_gain" => "Положительный эффект не доказан",
        _ => "Результат недоступен",
    }
}

fn rate(successes: i64, denominator: i64) -> String {
    if denominator < PRIVATE_MIN {
        "Скрыто: малая группа".into()
    } else {
        format!(
            "{successes}/{denominator} ({:.1}%)",
            successes as f64 / denominator as f64 * 100.0
        )
    }
}

pub(super) fn render(body: &mut String, report: Option<&ExperimentReport>) {
    body.push_str("<section aria-labelledby=\"surprise-reading-title\"><h2 id=\"surprise-reading-title\">Эксперимент: подтверждённое чтение</h2>");
    body.push_str("<p>Единица сравнения — назначенный аккаунт, включая не открывших подарок и отключивших сюрпризы. Набор длится 14 суток, наблюдение — 8 суток на участника. Итог подводится только после полного окна.</p>");
    body.push_str("<p>Новая книга: за первые 7 суток подтверждено завершение главы книги, не начатой до назначения. D1 — возврат к чтению через 24–48 часов, D7 — через 168–192 часа. Открытие подарка и клик по книге сюда не входят.</p>");
    let Some(report) = report else {
        body.push_str("<p>Эксперимент ещё не запускался.</p></section>");
        return;
    };
    let _ = write!(
        body,
        "<p data-surprise-experiment=\"{}\">Запуск №{}: {}</p><p>UTC: начало {}; набор до {}; окончание {}. Агрегат рассчитан: {}.</p>",
        escape_html(&report.decision),
        report.id,
        decision(&report.decision),
        escape_html(&report.started_at),
        escape_html(&report.enrollment_ends_at),
        escape_html(&report.ends_at),
        escape_html(&report.measured_at),
    );
    if report.counts.iter().any(|arm| arm.enrolled < PRIVATE_MIN) {
        body.push_str("<p>Значения скрыты: менее 20 аккаунтов хотя бы в одной группе.</p></section>");
        return;
    }
    body.push_str("<div style=\"overflow-x:auto\"><table><thead><tr><th>Группа</th><th>Назначено</th><th>Полное окно</th><th>Новая книга</th><th>Текущая глава</th><th>D1</th><th>D7</th><th>Отключение</th></tr></thead><tbody>");
    for (arm, name) in report
        .counts
        .iter()
        .zip(["Без подарков", "Мягкая", "Обычная"])
    {
        let _ = write!(
            body,
            "<tr><th>{name}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            arm.enrolled,
            arm.mature,
            rate(arm.meaningful, arm.mature),
            rate(arm.current_completed, arm.baseline_eligible),
            rate(arm.returned_d1, arm.mature),
            rate(arm.returned_d7, arm.mature),
            rate(arm.disabled, arm.mature),
        );
    }
    body.push_str("</tbody></table></div><p>Текущая глава оценивается только у участников, имевших незавершённую главу до назначения. Минимум для итоговой оценки — 1000 участников с полным окном в каждой группе; этого недостаточно само по себе для доказательства эффекта.</p>");
    if let Some(differences) = report.differences {
        for (delta, name) in differences.iter().zip(["Мягкая", "Обычная"]) {
            let _ = write!(
                body,
                "<p>{name} против контроля: {:+.2} процентного пункта; одновременный интервал: [{:+.2}; {:+.2}].</p>",
                delta.estimate * 100.0,
                delta.lower * 100.0,
                delta.upper * 100.0,
            );
        }
    }
    body.push_str("<p>Порог ухудшения чтения: 3 процентных пункта; рост отключений: 5 пунктов. Интервалы учитывают множественные сравнения; досрочное наблюдение не даёт права объявить улучшение. Завершение и аварийная остановка выключают подарки, а не переводят контроль в обычный режим. Включение после проверки результата — отдельное действие администратора.</p></section>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_recommendations::services::surprise_experiment::ArmCounts;

    #[test]
    fn absent_and_small_trials_never_expose_account_rows_or_rates() {
        let mut body = String::new();
        render(&mut body, None);
        assert!(body.contains("ещё не запускался"));
        let report = ExperimentReport {
            id: 1,
            state: "running".into(),
            decision: "collecting".into(),
            started_at: "<script>".into(),
            enrollment_ends_at: String::new(),
            ends_at: String::new(),
            measured_at: String::new(),
            counts: [ArmCounts::default(); 3],
            differences: None,
        };
        body.clear();
        render(&mut body, Some(&report));
        assert!(body.contains("Значения скрыты"));
        assert!(!body.contains("<table>"));
        assert!(!body.contains("<script>"));
        assert!(body.contains("&lt;script&gt;"));
    }
}
