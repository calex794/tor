//! Price and lifetime-commitment display for package owners.
use super::*;

pub(super) fn offer_form_html(layer: i64, offer: &LayerOwnerOfferState, csrf: &str) -> String {
    let old = match (offer.price_minor, offer.term_kind) {
        (Some(price), Some(kind)) => format!(
            "Текущая цена: {},{:02} ₽; {}.",
            price / 100,
            price % 100,
            if kind == LayerTermKind::Lifetime {
                "бессрочно"
            } else {
                "30 дней, ручное продление"
            }
        ),
        _ => "Цена ещё не опубликована.".to_owned(),
    };
    let price_value = offer.price_minor.map_or_else(String::new, |price| {
        format!("{},{:02}", price / 100, price % 100)
    });
    let options = if offer.term_kind == Some(LayerTermKind::Lifetime) {
        r#"<option value="lifetime">Бессрочно, включая будущие книги</option>"#
    } else {
        r#"<option value="prepaid_30d">30 дней, только ручное продление</option>
            <option value="lifetime">Бессрочно, включая будущие книги</option>"#
    };
    format!(
        r#"<section aria-labelledby="layer-offer-heading">
         <h2 id="layer-offer-heading">Цена и условия пакета</h2>
         <p>{old}</p>
         <p>Цена действует только для новых добровольных покупок.
         Старые оплаченные сроки сохраняются, автоматических списаний нет.
         Для продажи необходим непустой состав и подтверждённые доли доходов.</p>
         <p class="author-layers-page__important"><strong>Важно о бессрочном доступе:</strong>
         покупатель получает права также на будущие книги, добавленные в этот пакет.
         Удаление книги из нового предложения не отменяет уже полученные права.
         Это долгосрочное обязательство перед читателем.</p>
         <form method="post" action="/me/layers/{layer}/offer" class="author-layer-card__form">
           <div class="author-layer-card__fields">
             <div class="author-layer-card__field">
               <label for="layer-price">Цена в рублях</label>
               <input id="layer-price" name="price_rub" type="text"
                      inputmode="decimal" placeholder="Например, 149,00"
                      pattern="[0-9]+([,.][0-9]{{1,2}})?" required
                      value="{price_value}">
             </div>
             <div class="author-layer-card__field">
               <label for="layer-term">Вид доступа</label>
               <select id="layer-term" name="term_kind">{options}</select>
             </div>
           </div>
           <label class="author-layers-page__confirm">
             <input type="checkbox" name="confirm_pricing" value="yes" required>
             Подтверждаю публикацию новых условий продажи без изменения предыдущих оплат
           </label>
           <label class="author-layers-page__confirm">
             <input type="checkbox" name="confirm_lifetime" value="yes">
             Если выбран бессрочный доступ: подтверждаю обязательство открывать покупателям и будущие книги пакета
           </label>
           <input type="hidden" name="expected_version_id" value="{}">
           <input type="hidden" name="expected_offer_id" value="{}">
           <input type="hidden" name="csrf_token" value="{}">
           <button class="ui-button ui-button--primary" type="submit">Опубликовать условия</button>
         </form></section>"#,
        offer.current_version_id,
        offer.current_offer_id.unwrap_or(0),
        pages::escape_html(csrf),
    )
}
