# UI-12 — compact secondary book actions (script-free)

Status: Draft PR candidate, no deployment. Follow-up to UI-10B #1159 and
the zero-vote rating cleanup UI-11 #1162.

## Problem
The public book hero at 390 px uses two rows for Follow, referral share,
direct share and Report. Readers need the real reading and Library controls
visible first, without removing access to distinct existing actions.

## Scope
- Keep the primary reader, Library and batch-purchase links untouched.
- Keep Follow and verified author controls visible in the current secondary row.
- Group the existing referral, direct share and report links under one
  closed-by-default native HTML details/summary control, **no JavaScript**.
- Preserve the exact first-party href targets, order, SSR owner placeholder
  processing, strict `script-src 'none'`, keyboard Enter toggle and focus ring.
- Reuse all three existing presentation layouts and light/dark themes.
- Do not alter routes, role checks, permissions, payment, cookie values, S3,
  migrations, persistent data or production configuration.

## Evidence
- Rust SSR fixture checks all links remain unique, follow/owner placeholders
  survive, disclosure is initially closed and keyboard focus styling exists.
- Existing PostgreSQL real-book Chromium acceptance validates links, closed
  state, keyboard open/close and visible report link after expansion; the same
  journey continues through three layouts and desktop/mobile widths.
- Accept only after exact-head Repository rules, full Integration CI and
  post-merge verification on the exact merged commit.
