//! Native, script-free composition grid and owner-only impact review.
//! All text and posted IDs are escaped or server-validated.
use crate::pages;
use std::fmt::Write;
use tor_commerce::{
    domain::layers::{LayerOwnerOfferState, LayerTermKind, OwnedLayerSummary},
    repositories::layer_composition::{LayerBookTilePage, LayerChangePreview},
};

fn cover(id: i64, title: &str, has_cover: bool) -> String {
    if has_cover {
        format!(r#"<img src="/covers/{id}/card.webp" alt="" loading="lazy" decoding="async">"#)
    } else {
        let initial = title.chars().next().unwrap_or('К').to_string();
        format!(
            r#"<span class="package-tile__initial" aria-hidden="true">{}</span>"#,
            pages::escape_html(&initial),
        )
    }
}

pub(super) fn detail(
    layer: &OwnedLayerSummary,
    books: &LayerBookTilePage,
    offer: &LayerOwnerOfferState,
    csrf: &str,
    query: &str,
    editable: bool,
) -> (String, String, String) {
    let title = pages::escape_html(&layer.title);
    let hero_cover = books
        .hero_cover_id
        .map(|id| cover(id, "", true))
        .unwrap_or_else(|| {
            r#"<span class="package-tile__initial" aria-hidden="true">К</span>"#.to_owned()
        });
    let tariff = match (offer.price_minor, offer.term_kind) {
        (Some(price), Some(kind)) => format!(
            "{},{:02} руб. · {}",
            price / 100,
            price % 100,
            if kind == LayerTermKind::Lifetime {
                "бессрочно + будущие книги"
            } else {
                "30 дней без автопродления"
            },
        ),
        _ => "Цена ещё не опубликована".to_owned(),
    };
    let status = match layer.status.as_str() {
        "active" => "Опубликован",
        "hidden" => "Скрыт",
        _ => "Заблокирован",
    };
    let reader_link = if layer.status == "active" && offer.current_offer_id.is_some() {
        format!(
            r#"<a class="text-link" href="/layers/{}">Посмотреть страницу для читателей</a>"#,
            layer.layer_id,
        )
    } else {
        "<span class=\"package-hero__note\">Страница для читателей появится после публикации предложения.</span>".to_owned()
    };
    let hero = format!(
        r#"<section class="package-hero" aria-labelledby="package-title">
            <div class="package-hero__cover">{hero_cover}</div>
            <div class="package-hero__copy">
              <p class="author-layer-card__eyebrow">Пакет №{id} · {status}</p>
              <h1 id="package-title">{title}</h1>
              <p>{description}</p>
              <strong>{tariff}</strong>
              <p class="package-hero__note">Показана обложка одной из книг пакета. Персональная обложка пакета пока не загружается.</p>
              {reader_link}
            </div>
           </section>"#,
        id = layer.layer_id,
        description = if layer.description.trim().is_empty() {
            "Добавьте описание: какие книги входят в комплект и для кого он создан.".to_owned()
        } else {
            pages::escape_html(&layer.description)
        },
    );
    let metadata = if layer.status == "blocked" {
        "<p>Пакет заблокирован; оформление не редактируется.</p>".to_owned()
    } else {
        let active = if layer.status == "active" {
            " selected"
        } else {
            ""
        };
        let hidden = if layer.status == "hidden" {
            " selected"
        } else {
            ""
        };
        format!(
            r#"<details class="package-section__edit">
              <summary>Редактировать название, описание и видимость</summary>
              <form method="post" action="/me/layers/{id}/metadata" class="author-layer-card__form">
                <div class="author-layer-card__fields">
                  <div class="author-layer-card__field author-layer-card__field--full">
                    <label for="package-title-edit">Название</label>
                    <input id="package-title-edit" name="title" maxlength="120" required value="{title}">
                  </div>
                  <div class="author-layer-card__field author-layer-card__field--full">
                    <label for="package-description">Описание</label>
                    <textarea id="package-description" name="description" maxlength="2000" rows="3">{description}</textarea>
                  </div>
                  <div class="author-layer-card__field">
                    <label for="package-visibility">Видимость</label>
                    <select id="package-visibility" name="status">
                      <option value="active"{active}>Опубликован</option>
                      <option value="hidden"{hidden}>Скрыт для новых покупок</option>
                    </select>
                  </div>
                </div>
                <input type="hidden" name="expected_revision" value="{revision}">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <button class="ui-button ui-button--primary" type="submit">Сохранить оформление</button>
              </form>
            </details>"#,
            id = layer.layer_id,
            description = pages::escape_html(&layer.description),
            revision = layer.metadata_revision,
            csrf = pages::escape_html(csrf),
        )
    };

    if !editable {
        return (
            hero,
            metadata,
            "<p>Пакет скрыт. Состав и цену можно просматривать, но изменить состав пока нельзя.</p>"
                .to_owned(),
        );
    }
    let mut tiles = String::new();
    let mut visible = Vec::new();
    let mut included = Vec::new();
    for book in &books.books {
        visible.push(book.id.to_string());
        if book.included {
            included.push(book.id.to_string());
        }
        let checked = if book.included { " checked" } else { "" };
        let included_class = if book.included { " is-included" } else { "" };
        let marker = if book.included {
            "В пакете"
        } else {
            "Добавить"
        };
        write!(
            tiles,
            r#"<label class="package-tile{included_class}">
                <input class="package-tile__checkbox" type="checkbox"
                       name="chosen_{id}" value="1"{checked}>
                <span class="package-tile__cover">{cover}</span>
                <span class="package-tile__title">{name}</span>
                <span class="package-tile__marker">{marker}</span>
               </label>"#,
            id = book.id,
            cover = cover(book.id, &book.title, book.has_cover),
            name = pages::escape_html(&book.title),
        )
        .expect("write tile");
    }
    if tiles.is_empty() {
        tiles.push_str("<p>Не найдено подходящих книг. Попробуйте другое название.</p>");
    }
    let mut navigation = String::new();
    if let Some(after) = books.next_after {
        write!(
            navigation,
            r#"<a class="ui-button ui-button--ghost" href="/me/layers/{id}/books?after={after}{suffix}">Следующие книги</a>"#,
            id=layer.layer_id,
            suffix=super::encoded_search_suffix(query),
        )
        .expect("write next page");
    }
    let grid = format!(
        r#"<div class="package-section__head">
             <div><h2>Выбор книг</h2>
             <p>Галочка означает, что книга входит в пакет. Изменения применяются только к книгам, показанным на этой странице; остальные остаются без изменений.</p></div>
           </div>
           <form method="get" action="/me/layers/{id}/books" class="author-layers-page__search">
             <label for="package-search">Поиск по собственным книгам</label>
             <input id="package-search" name="q" type="search" maxlength="100" value="{query}"
                    placeholder="Поиск по названию">
             <button class="ui-button ui-button--ghost" type="submit">Найти</button>
             <a class="text-link" href="/me/layers/{id}/books">Сбросить</a>
           </form>
           <form method="post" action="/me/layers/{id}/books" class="package-grid-form">
             <input type="hidden" name="operation" value="preview">
             <input type="hidden" name="expected_version_id" value="{version}">
             <input type="hidden" name="visible_csv" value="{visible}">
             <input type="hidden" name="included_csv" value="{included}">
             <input type="hidden" name="csrf_token" value="{csrf}">
             <div class="package-grid">{tiles}</div>
             <div class="package-grid__actions">
               <button class="ui-button ui-button--primary" type="submit">Проверить изменения</button>
               <span class="package-grid__live">Добавить: <strong class="package-grid__added"></strong> · Убрать: <strong class="package-grid__removed"></strong></span>
               <span>Сначала появится список добавленных и удалённых книг. До окончательного подтверждения состав не изменится.</span>
             </div>
           </form>
           <nav class="package-grid__pagination" aria-label="Страницы книг">{navigation}</nav>"#,
        id = layer.layer_id,
        query = pages::escape_html(query),
        version = offer.current_version_id,
        visible = visible.join(","),
        included = included.join(","),
        csrf = pages::escape_html(csrf),
    );
    (hero, metadata, grid)
}

pub(super) fn review(
    layer: i64,
    preview: &LayerChangePreview,
    csrf: &str,
) -> axum::response::Html<String> {
    let mut adds = String::new();
    let mut removes = String::new();
    for book in &preview.additions {
        write!(adds, "<li>{}</li>", pages::escape_html(&book.title)).expect("add title");
    }
    for book in &preview.removals {
        write!(removes, "<li>{}</li>", pages::escape_html(&book.title)).expect("remove title");
    }
    if adds.is_empty() {
        adds.push_str("<li>Новых книг нет</li>");
    }
    if removes.is_empty() {
        removes.push_str("<li>Ничего не удаляется</li>");
    }
    let added_csv = preview
        .additions
        .iter()
        .map(|b| b.id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let removed_csv = preview
        .removals
        .iter()
        .map(|b| b.id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let contents = format!(
        r#"<section class="me-page author-workspace author-layers-page">
           <div class="container account-layout">
             <!--ACCOUNT_NAVIGATION-->
             <div class="account-main">
               <div class="package-review__shade" aria-hidden="true"></div>
               <dialog open class="package-review" aria-labelledby="package-review-title">
                 <p class="author-layer-card__eyebrow">Предпросмотр · версия {version}</p>
                 <h1 id="package-review-title">Подтвердить изменения пакета</h1>
                 <div class="package-review__columns">
                   <section><h2>Добавим: {add_count}</h2><ul>{adds}</ul></section>
                   <section><h2>Уберём: {remove_count}</h2><ul>{removes}</ul></section>
                 </div>
                 <h2>Что произойдёт с читателями</h2>
                 <p><strong>{active}</strong> — действующих покупателей пакета.
                 Из них <strong>{lifetime}</strong> имеют бессрочный доступ.</p>
                 <p>При удалении книг <strong>0 существующих оплаченных прав будет отозвано</strong>.
                 Текущие покупатели (до {active} человек) сохранят доступ к уже приобретённым книгам.</p>
                 <p>При добавлении до <strong>{active} действующих покупателей</strong>
                 получат новые права на добавляемые книги через этот пакет без дополнительной оплаты.
                 Реальная доступность чтения зависит от публикации книги и возрастного уровня.</p>
                 <p class="package-review__caution">Изменение затронет будущее предложение для новых покупателей.
                 Если число действующих покупателей изменится до подтверждения, сервер попросит повторить проверку.</p>
                 <form method="post" action="/me/layers/{layer}/books">
                   <input type="hidden" name="operation" value="apply">
                   <input type="hidden" name="expected_version_id" value="{version}">
                   <input type="hidden" name="add_csv" value="{added_csv}">
                   <input type="hidden" name="remove_csv" value="{removed_csv}">
                   <input type="hidden" name="active_buyers" value="{active}">
                   <input type="hidden" name="lifetime_buyers" value="{lifetime}">
                   <input type="hidden" name="csrf_token" value="{csrf}">
                   <label class="author-layers-page__confirm">
                     <input type="checkbox" name="confirm_composition" value="yes" required>
                     Подтверждаю изменение состава и сохранение ранее оплаченных прав
                   </label>
                   <div class="author-layer-card__actions">
                     <button class="ui-button ui-button--primary" type="submit">Подтвердить и сохранить</button>
                     <a class="ui-button ui-button--ghost" href="/me/layers/{layer}/books">Отменить</a>
                   </div>
                 </form>
               </dialog>
             </div>
           </div>
         </section>"#,
        version=preview.version_id,
        add_count=preview.additions.len(),
        remove_count=preview.removals.len(),
        active=preview.active_buyers,
        lifetime=preview.lifetime_buyers,
        csrf=pages::escape_html(csrf),
    ).replace(
        "<!--ACCOUNT_NAVIGATION-->",
        &pages::account_navigation("/me/layers", csrf),
    );
    pages::private_account_layout(
        "Подтверждение состава пакета",
        "Проверка изменений и сохранения оплаченных прав.",
        &contents,
    )
}
