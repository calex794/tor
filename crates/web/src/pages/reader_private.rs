//! Private, account-only reader UI. No client-side or inferred access grants.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::progress::{
    reader_blocks, valid_text_place, ReadingPlace, SavedPlaceState,
};

/// Account-only reading: the browser can save a deterministic point in an
/// immutable, currently published, free chapter without requiring JavaScript.
pub fn private_reader(
    chapter_id: u64,
    chapter: &tor_reading::services::PublicChapterPage,
    csrf: &str,
    saved: SavedPlaceState,
    expected_revision: i64,
    completed: bool,
) -> Html<String> {
    let blocks = reader_blocks(&chapter.body_markdown);
    // Do not infer completion from a GET or from a saved paragraph marker.
    // Only the final non-blank server-parsed block offers the explicit action.
    let finish = blocks
        .iter()
        .enumerate()
        .rev()
        .find(|(_, text)| !text.trim().is_empty())
        .and_then(|(index, text)| {
            let place = ReadingPlace {
                block_index: i32::try_from(index + 1).ok()?,
                char_offset: i32::try_from(text.encode_utf16().count()).ok()?,
            };
            valid_text_place(place, &blocks).ok().map(|()| place)
        });
    let complete_form = if completed {
        "<p class=\"reader-complete\" role=\"status\">Глава уже отмечена прочитанной.</p>"
            .to_owned()
    } else {
        finish.map_or_else(String::new, |place| {
        format!(
            r#"<div class="reader-complete">
              <p>Закончили главу? Отметьте её прочитанной, чтобы добавить книгу в личный рейтинг.</p>
              <form method="post" action="/read/{chapter_id}/complete">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="version_id" value="{version_id}">
                <input type="hidden" name="block_index" value="{block_index}">
                <input type="hidden" name="char_offset" value="{char_offset}">
                <input type="hidden" name="expected_revision" value="{expected_revision}">
                <button type="submit" class="ui-button ui-button--primary">Глава прочитана</button>
              </form>
            </div>"#,
            csrf = escape_html(csrf),
            version_id = chapter.version_id,
            block_index = place.block_index,
            char_offset = place.char_offset,
        )
        })
    };
    let mut sections = String::new();
    for (index, source) in blocks.iter().enumerate() {
        let block_index = index + 1;
        let markdown = tor_reading::domain::renderer::render(source);
        write!(
            sections,
            r#"<section class="reader-piece" id="p-{block_index}" aria-label="Фрагмент {block_index}">
                 <div class="reader-piece__text reader-markdown">{markdown}</div>
                 <form class="reader-piece__form" method="post" action="/read/{chapter_id}/place">
                    <input type="hidden" name="csrf_token" value="{csrf}">
                    <input type="hidden" name="version_id" value="{version_id}">
                    <input type="hidden" name="block_index" value="{block_index}">
                    <input type="hidden" name="char_offset" value="0">
                    <button type="submit" class="reader-save-button" aria-label="Сохранить после фрагмента {block_index}">Сохранить место здесь</button>
                 </form>
               </section>"#,
            csrf = escape_html(csrf),
            version_id = chapter.version_id
        ).expect("write reader block");
    }
    let saved_link = match saved {
        SavedPlaceState::Current(p) if valid_text_place(p, &blocks).is_ok() => format!(
            r##"<a class="reader-resume__link" href="#p-{}">Перейти к сохранённому месту</a>"##,
            p.block_index
        ),
        SavedPlaceState::PreviousVersion => concat!(
            "Глава обновлена. Сохранённое место относится к предыдущей версии. ",
            "Прочитайте новую версию и сохраните место заново."
        )
        .to_owned(),
        _ => "Место ещё не сохранено".to_owned(),
    };
    let prev = chapter
        .previous_chapter_id
        .map(|id| format!(r#"<a rel="prev" href="/read/{id}">Предыдущая глава</a>"#))
        .unwrap_or_default();
    let next = chapter
        .next_chapter_id
        .map(|id| format!(r#"<a rel="next" href="/read/{id}">Следующая глава</a>"#))
        .unwrap_or_default();
    let navigation = format!(
        r#"<nav class="chapter-navigation" aria-label="Переход между главами">{prev}{next}</nav>"#
    );
    let body = format!(
        r#"<section class="section reader-page reader-private" aria-labelledby="reader-title">
            <div class="container reader-width">
                <nav class="breadcrumbs" aria-label="Навигационная цепочка">
                    <a href="/me/library">Моя библиотека</a>
                    <span aria-hidden="true">/</span>
                    <a href="/books/{book_id}">{book_title}</a>
                </nav>
                <p class="eyebrow">Читаю с сохранением · Версия {version_no}</p>
                <h1 id="reader-title">{chapter_title}</h1>
                <p class="reader-page__subtitle">После нужного фрагмента нажмите «Сохранить место здесь». 
                  При следующем входе книга появится в библиотеке.</p>
                <div class="reader-resume" role="status">{saved_link}</div>
                {navigation}
                <article class="chapter-content reader-private__content" aria-label="Текст главы">
                    {sections}
                </article>
                {complete_form}
                {navigation}
                <p class="reader-page__return"><a class="text-link" href="/me/library">К моей библиотеке</a></p>
            </div></section>"#,
        book_id = chapter.book_id,
        book_title = escape_html(&chapter.book_title),
        version_no = chapter.version_no,
        chapter_title = escape_html(&chapter.chapter_title)
    );
    layout(
        &chapter.chapter_title,
        "Чтение с сохранением места в личной библиотеке.",
        &body,
    )
}
