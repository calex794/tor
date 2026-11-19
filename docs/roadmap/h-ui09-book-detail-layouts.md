# UI-09 — три различимых компоновки карточки книги

Status: PR candidate based on main@5a915ec7. No deploy.

## Product decision

The approved TOR UI concept has three layout styles (showcase/library/minimal)
with independent color themes and reader preferences. UI-07 removed decorative
introductions and preserved real data, but book detail did not yet use meaningful
layout differences between the three styles.

## Implementation

- **Showcase**: medium cover beside the title and primary actions, readable editorial
  presentation with no oversized hero or fake cards.
- **Library**: smaller cover, dense bibliographic hierarchy, action block below the
  metadata, tighter TOC sections and modest card geometry.
- **Minimal**: title-first layout, compact right-aligned cover and border-only sections
  instead of large framed panels.
- At <= 760px, all three use distinct, deliberately compact grid compositions.
- Style changes only CSS. SSR book IDs, title, cover, chapter TOC, ratings, referrals,
  bookmarks, paid actions and editorial promotions are not duplicated or removed.
- Existing appearance setting, no-JS fallback, independent theme, catalog layout,
  reader settings and security policies remain unchanged.

## Acceptance

- Demo Chromium at 320, 390, 768 and 1440px: one title and cover slot, an actionable
  primary control, unchanged SSR hrefs, no horizontal overflow.
- On every viewport, computed CSS composition signatures are distinct for all
  three styles; screenshot evidence for 390/1440px.
- Full PR Repository rules and Integration CI on exact head, then post-merge
  CI on current main. VPS, Yandex S3 and persistent PostgreSQL are out of scope.
