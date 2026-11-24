# REUSE-08A — common S049 appeal form markup

H/E. Three author appeal forms and three administrator decision forms in
the existing book_shares router have identical markup except for their
explicit Book, FREE and PAID destinations. They now use one typed,
presentation-only renderer for each form and a shared status label.
The enum only selects static paths. The callers still decide when a form
is shown and supply the exact hide-event/appeal ID, CSRF and freshly
generated command key. The renderer escapes attribute values. No new
forms, permissions, role checks, repository calls or request parsing.

Preserve the existing HTML (including whitespace, action URL, field
names/order/limits and button values), distinct author and administrator
operations, status text and unknown fallback. All six POST handler
bodies are byte-identical to the base. The REUSE-02B session import,
RIL proof, actual PG scope checks, visibility logic, page titles and
separate repositories remain unchanged. No JS, CSS, route, database or
dependency change. Three new unit tests cover all six form variants,
attribute escaping and status labels. Acceptance requires exact-head
Rust and real PG S049 tests and full required CI, then post-merge.
No deployment.
