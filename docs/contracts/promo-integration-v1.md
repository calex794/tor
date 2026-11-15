# PROMO-00/P1 — единые специальные предложения TOR: аудит и первый срез

Дата аудита: 15.11.2026. Основание: [Notion «Промо»](https://app.notion.com/p/41932a84beec81e4b1b1d21745360b2b).
Исходный main: [139eb078359cc956ae76543364a565cf811306bb](https://github.com/calex794/tor/commit/139eb078359cc956ae76543364a565cf811306bb).
Это конкретизация F/S042/S043, editorial FLOW-08 и F-SURPRISE; **не** третья система рекламы.

## P0 — проверенные узлы действующего кода

| Узел | Факт | Решение |
| --- | --- | --- |
| `crates/ads/src/domain/editorial.rs` | Кампания строго связана с одной Book и одним PromotionSlot; даты, priority и revision нормализуются | Не вводить Boosty URL вместо book_id |
| `crates/ads/src/repositories/editorial/{commands,serving}.rs` | PostgreSQL CAS, admin session, audit, recheck Book ACL/Hide/age, приоритет | Сохранять серверные проверки и replay |
| `crates/contracts/src/exposure.rs` | Typed PromotionSlot + анонимное наблюдение BookCard; обязательно book_id>0 | Внешний оффер требует отдельного типизированного наблюдения/агрегата, без подделки Book |
| `crates/web/src/editorial_http/{admin,display,render}.rs` | Native SSR, CSRF/Origin, превью без показов, slot-aware BookCard | Расширять существующую админку для Book-only; внешние офферы отдельным срезом |
| `crates/web/src/homepage_updates.rs` / `templates/home.html` | Главная прикрепляет home_featured и использует маркер | Пилотная полоса над витринами, fallback без второй редакционной рекламы |
| `crates/web/src/recommendation_http/decorate.rs` | После бесплатной главы editorial и organic уже составляют одну физическую полку из пяти | Не добавлять отдельный рекламный ряд и не менять квоты 50/40/10 |
| `crates/web/src/auth/surprise_http.rs` / `assets/surprise.js` | Отдельный default-OFF planner/claim/open, ограничения безопасной зоны, нет внешнего CTA | Позднее подключать промо только в content selector после открытия, не менять вероятность появления |
| `docs/contracts/editorial-promotion-v1.md` | Содержит действующий FLOW-08 договор, три первоначальных места | Расширить аддитивно, не менять исторические гарантии |
| `migrations/202611246853_editorial_promotion.sql` | CHECK разрешал ровно три Book placement | Новая миграция, старую не переписывать |

## P1a — фактическая первая поставка этого PR

- Добавить четвёртый Book-only slot `home_promo_strip` без нового финансового контура.
- Существующая admin SSR-форма создаёт это назначение по Book ID, времени и priority.
- SSR на главной выбирает одну карточку промополосы (кликабельный переход только к публичной книге), или использует прежний home_featured как fallback.
- Нельзя одновременно показывать две редакционные рекламы на главной; Book ACL/возраст/Hide/расписание, нулевые денежные движения и общая защита FLOW сохраняются.
- Выдача под флагом `TOR_EDITORIAL_PROMOTION_ENABLED=1`, по умолчанию OFF.
- HTML title экранирован, цифровой Book URL собран сервером, внешних ссылок нет.
- Появление полосы не добавляет новой JS-библиотеки и не мешает обычной главной.
- Прогресс: реализация в Draft PR. Никакого деплоя или рабочих миграций.

## P1b — следующий вертикальный срез (ещё НЕ реализован)

1. Типизированная модель `PromoOffer` отдельно от Book-only `EditorialCampaign` с `tor_book`, `tor_package`, `boosty_external`, immutable terms snapshot и несколькими creatives/placements.
2. Строгая проверка внешнего URL через стандартный URL parser: HTTPS и точный allowlist Boosty, без userinfo, нестандартного порта, fragment и script; безопасная ссылка, no secrets in logs.
3. Админский SSR create/update/pause/preview по существующим A/RIL/CSRF/CAS/audit инвариантам; разные экраны без raw HTML.
4. Только активные URL, время и ручная сверка фактической акции Boosty; не утверждать внешний API/eligibility или реальную покупку по клику.
5. Выдача `home_promo_strip` + `chapter_end_free` по общему placement policy без замены книжной полки. Обезличенные shown/click/outbound агрегаты без фальшивых покупок.
6. Реальный PG/HTTP/Chromium 390/1440/JS-off/role-revocation/закрытые книги и rate-limit, отдельный CI на exact head.

## P2 — безопасное подключение «Случайных открытий»

Использовать существующие `surprise_content`, планировщик и daily cap. Promo — только потенциальный контент после явного открытия; пилотная доля прямого промо не более 25% раскрытий. Не рекламировать в закрытых платных главах, не менять вероятность по истории покупок, не превращать подарок в призовую механику. Отдельный Chromium/PG эксперимент с контрольной группой, сохранёнными правилами отключения и негативными метриками.

## Проверки для P1a

- Unit: slot parser, root/host rule, один escaped promo strip вместо нескольких карточек.
- PostgreSQL: включение и пауза назначения, guest Book visibility, отсутствие нужды в host Book; миграции применить дважды в CI.
- HTTP: admin/reader/session/Origin/CSRF и реальные маршруты из FLOW-08 должны остаться зелёными.
- Chromium: новая промополоса в 390/1440, доступность кнопки и отсутствие дублирования редакционных мест — требуется отдельная приёмка до слияния.
- Внешний выпуск: запрещён до отдельного ручного решения о проверенном main SHA.

## Красные линии

Не менять purchased V1/Layer rights; не открывать hidden/draft text; не считать render/view/click = purchase; не создавать ledger по показу/Boosty trial; не возобновлять retired индивидуальный REC/FLOW collection; не замещать `chapter_end_free` и квоты органики.

Связанные спецификации: [editorial promotion](editorial-promotion-v1.md), [ads](../roadmap/ads-execution-contract.md), [attention](../roadmap/rec-attention-unified-plan.md), [FLOW](flow-rec-runtime-retirement-v2.md).
