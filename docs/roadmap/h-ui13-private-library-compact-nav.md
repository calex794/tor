# UI-13 — compact, fully visible private library navigation

Status: PR candidate; no production deployment. Based on real PG/Chromium
screenshots from main@9fb1a908, which show the purchased tab clipped at 390px.

## Defect and constraints

The library header repeats decorative subtitles and displays the functional
"Рекомендации друзей" link alone on a separate row. Three independent
layout controls wrap to an unnecessary extra row; the fourth "Куплено" tab
is clipped on narrow viewports. The page is private and must keep
`script-src 'none'`, server-only GET links, secure CSRF forms, purchases,
saved positions, deleted books and the full 4-tab navigation.

## Change

- Remove only the superfluous "Моё чтение" kicker and inspirational subtitle.
- Keep "Найти книгу" and "/library/shared" together in the action area;
  keep conditional purchases / layer links untouched, hide empty wrapper.
- Render four equal-width accessible tabs on mobile, wrapping text at 320px
  instead of clipping last tab. Three GET layout controls fit a single row.
- Reduce nonfunctional spacing; keep desktop styles and library content.
- SSR unit checks all four links and absence of decorative content.
- Real PostgreSQL + Chromium checks 320 / 390 / 1440 all four tab bounding
  boxes inside the native nav, three layout choices, no horizontal overflow,
  and friend link; existing CSP/cookies/CSRF/bookmark tests continue.

Acceptance: exact-head Repository rules and full Integration CI, then
post-merge on main. No VPS, Yandex S3 or persistent PostgreSQL.
