//! One administrative entry point for the distinct FLOW sources.
pub(crate) fn links(days: i32) -> String {
    let days = if [1, 7, 30, 90].contains(&days) { days } else { 7 };
    format!(r#"<nav class="audience-books-pagination" aria-label="Отчёты аудитории">
<a href="/admin/audience?days={days}">Аудитория и часы</a>
<a href="/admin/audience/reading?days={days}">Подтверждённое чтение</a>
<a href="/admin/audience/exposure?days={days}">Показы карточек</a>
<a href="/admin/audience/seo?days={days}">Google и Яндекс</a>
<a href="/admin/audience/signals?days={days}">Сигналы</a>
<a href="/admin/audience/pages">Публичные страницы</a>
<a href="/admin/promotion/editorial">Редакционное продвижение</a>
<a href="/me/recommendations/contextual">Контекстные рекомендации</a>
</nav>"#)
}
