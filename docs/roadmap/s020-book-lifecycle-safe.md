# S020 — NEwBOO lifecycle vs publication

This slice ports safe Book lifecycle metadata from the NEwBOO
`books_settings.py` `lifecycle_status` field.

- Values: `in_progress`, `completed`, `on_hiatus`, `abandoned`. The initial value is `in_progress`.
- `pending_deletion` is present in the stored enum but cannot be entered or left by the basic editor. NEwBOO requires a distinct owner step-up/password and deletion notification flow. That feature is not implemented here.
- Scoped editor access: current owner, active translator, active Book moderator for the source Book. Another Book's membership, inactive membership, held/deleted Book: no access.
- Mutations lock the Book row, then the member row within a single PostgreSQL transaction, and enforce an independent lifecycle revision (HTTP conflict on stale writes).
- No code path here changes `project_status`, `visibility`, `access_level`, paid grants, financial ledger, purchased chapters, or transfer rights.
- NEwBOO site-level moderation visibility transitions require different authority and must not be granted to Book moderators merely through this route.
- S020 remains partial after this slice: deletion step-up, visibility/access-level parity, member settings navigation and end-to-end acceptance remain separate.

Reference: `HeisLuka/newboo/backend/app/web/routes/books_settings.py` at `17125ef758a791e9630505c14d24f7fe65aed228`.
