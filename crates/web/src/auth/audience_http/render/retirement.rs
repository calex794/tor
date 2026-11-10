//! Keep retired individual measurements separate from current aggregate evidence.
use crate::pages::escape_html;
use std::fmt::Write;

pub(super) const CURRENT: &str = "Исторические записи выбранного интервала";
pub(super) const PREVIOUS: &str = "Исторические записи предыдущего интервала";

pub(super) fn notice(body: &mut String, measurements: &str) {
    write!(
        body,
        r#"<section class="book-section-card" data-testid="collection-retired">
        <h2>Текущее измерение</h2>
        <p><strong data-current-measurement="unavailable">Не измеряется</strong> — {}.</p>
        <p>Индивидуальный сбор посещений, переходов и возвращений прекращён.
        Текущее значение не подменяется нулём.</p>
        <p>Исторические показатели используют сохранённые наблюдения прежней выборки аккаунтов
        с отдельным согласием и исходные границы их периодов. Они не описывают
        текущую посещаемость всего сайта.</p>
        <p>Исторические итоги могут уточняться при обработке оставшихся записей
        и удалении данных по сроку хранения. Отсутствие нового наблюдения
        не означает, что посетитель не пришёл или читатель не вернулся.</p></section>"#,
        escape_html(measurements)
    )
    .expect("String");
}
