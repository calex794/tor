# UI-01 — переключение оформления TOR

Status: UI-01 accepted and merged in PR #1126 at main@7fd52aac. Existing server-rendered book data and all access rules remain unchanged.

## Behavior
- A single, keyboard-operable header picker provides homepage style (showcase/library/minimal), global color (system/light/dark), card density and an independent catalog layout (grid/list/large covers).
- Defaults: showcase, system, normal, grid; no-JavaScript uses the original layout and operating-system color theme. Controls are hidden until the script initializes.
- A bounded, versioned localStorage record (torAppearance.v1) applies before CSS, gracefully handles failures and untrusted values, and synchronizes across browser tabs.
- Reader pages use their own shell and independent display preferences, unaffected by the picker.
- HTML, book links, recommendations and exposure attributes are not re-rendered, moved, duplicated or removed. Changes affect CSS only; there is no server preference, database migration or new personal data.

## Acceptance
- Rust: UI-01 shell assertions in crates/web/src/pages/tests.rs.
- Node: node --test tools/tor_appearance.test.cjs.
- Browser: verify keyboard focus, OS dark with explicit light, OS light with explicit dark, default no-JS fallback; desktop 1440 and widths 768/390/320; three homepage modes and three catalog views; check no horizontal scroll and unchanged destination URLs.
- Regression: ensure editorial and recommendation impression tracking remains tied to actual displayed cards.

## Follow-up
- UI-02: unify appearance design tokens and eliminate remaining hardcoded styles; add layout-specific component templates only where CSS is inadequate.
- UI-03: independent reading-library layout and authenticated cross-device preference sync.
- UI-04: compare variants using real browser and accessibility acceptance before enabling layout reordering. This first slice deliberately does not reorder functional DOM sections.

## UI-02A — личная библиотека: семантические цветовые токены

- Replace legacy green fallback colors in library controls/cards with guaranteed shared theme tokens.
- Introduce a semantic danger-ink token for the removal hover state, including system dark and explicit light/dark.
- Preserve decorative book-cover gradients rather than recoloring artwork, as this is content styling rather than interface chrome.
- No changes to browser preferences, content data, auth, database, reader controls or chapter grants.
- Acceptance: structural Node contract, existing responsive Chromium and real-reader regression suite.

## UI-05 — настраиваемый порядок секций главной (candidate)

- Состояние: отдельный PR на основе `main@8cb93766`; принять только после Node, Rust, реального PostgreSQL/Chromium и полного CI.
- Переставляемые секции: недавно добавленные, обновления, редакционная подборка, набирающие читателей, жанры. Поиск, spotlight и промополоса остаются в исходном месте. Отсутствующие/пустые секции не получают доступных кнопок.
- Параметр `torHomeOrder.v1` — локальная полная перестановка пяти фиксированных идентификаторов. Любая неверная, повторная, неполная или недоступная запись сбрасывается в исходный порядок. Нет SQL, cookies, профилирования, персональных данных или новых permission checks.
- Перемещение выполняется `appendChild` существующих секций, не `order` и не `innerHTML`; фокус и Tab-очерёдность совпадают с визуальным DOM. Ссылки книг, авторизованные карточки, editorial кампании и выданные сервером индивидуальные `data-flow-proof` остаются прежними. Фактический view по-прежнему определяет независимый IntersectionObserver (60% / 1010 ms), а не изменение настроек.
- Без JavaScript порядок SSR неизменён, средства перестановки скрыты. Частные страницы и отдельный reader layout не приобретают бизнес-изменений.
- Проверки: `node --test tools/tor_appearance.test.cjs` включает UI-05; `cargo test -p tor-web ui_05_`; в `tools/editorial_pg_browser.cjs` подтверждается реальная PostgreSQL редакционная карточка, перестановка DOM, сохранение её proof/href, persistence и reset.

## UI-06 — компактная главная без декоративного повторного hero (19.11.2026)

Источник: [принятое решение и прототип v0.4 в Notion](https://app.notion.com/p/41d32a84beec81ab8486e95538817477).

- Убрать повторно выведенную карусель новых книг перед основным списком: это были те же public rows, не редакционная или оплаченная кампания.
- Сохранить серверные `HOME_PROMO_STRIP`, `EDITORIAL_PROMOTION`, реальные book IDs/cover URLs, FLOW exposure и прежнюю проверку прав. Никаких новых рекламных слотов, SQL, миграций, S3 или JavaScript для приватных страниц.
- На первом экране оставить доступный поиск и сразу реальные недавно добавленные книги; при пустом каталоге — честный empty-state без пустого hero.
- Showcase: исходная горизонтальная полка; Library: сетка + жанры из уже доступных public rows, на мобильном доступные горизонтальные ссылки; Minimal: компактные строковые карточки. Все режимы используют одни и те же SSR карточки.
- UI-05 порядок пяти пользовательских секций не менять. Глобальные темы/плотность и независимая читалка сохраняются.
- Приёмка: Rust home tests, Node `node --test tools/tor_appearance.test.cjs`, реальный Chromium 320/390/768/1440 × 3 стиля × темы, no-JS, клавиатура, URLs и отсутствие дублированных home spotlight impressions. Проверить отдельно PostgreSQL-backed реальную выдачу и post-merge на exact SHA. Не деплоить вместе с PR.


## UI-07 — полезный первый экран каталога и карточки книги (19.11.2026)

- Убрать неинформативную подпись «Библиотека платформы» и повторное объяснение поиска; оставить компактный H1 «Каталог», серверную форму и реальные карточки.
- Убрать декоративный kicker «Книга» с карточки; сократить пустые отступы. Полное имя книги, автор/жанры, обложка, основное чтение/покупка, оглавление, реальные редакционные размещения и права остаются.
- Хлебные крошки остаются доступными: ссылка на главную и каталог видны, текущая книга сохраняется в accessible tree без визуального повторения длинного заголовка.
- Все три style/theme пресета работают на общем SSR; настройки читалки, Content-Security-Policy, купленные V1, SEO robots policy и POST/CSRF не менять.
- Приёмка: Node contract `tools/tor_appearance.test.cjs`, Chromium `tools/tor_appearance_browser_smoke.cjs` на 320/390/768/1440, no-JS каталог, PostgreSQL/HTTP и полный CI exact SHA; после слияния отдельный post-merge. VPS, S3 и PG-миграции не затрагивать.
