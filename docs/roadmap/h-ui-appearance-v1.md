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
