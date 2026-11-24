# REUSE-02B — share S049 browser-session authentication

Scope: H/I, S049 (A/E contracts unchanged). Seven modules in the existing
`book_shares` router contain byte-identical `session_user` functions:
book complaints, Book/free/paid comment appeals, free comment reports,
and free/paid review queues. 22 registered GET/POST bindings use the helper.
Replace the seven definitions with one parent implementation and explicit
imports. The child handler bodies, routing, repository operations, and
all SQL are unchanged. Share recommendation reports deliberately retain
their distinct `session_digest` path.

No CSRF/Origin consolidation in this slice: POST forms keep CSRF-first
precedence, unlike the REUSE-02A strict JSON grant paths. All role checks,
transactional revalidation, separate free/paid rights, and audit records
remain in their original repositories. Session helper does not grant
any moderation or reading right.

RIL-08 accepts the exact reviewed route/method/handler/policy bindings,
checks the live import and the unchanged parent helper, and requires
immediate rejection before repository operations, preserving the existing
repository/role proofs. Mutation tests cover disabled live session,
failure branches, missing import, ignored denial, and removed Origin.
Acceptance: final SHA Repository rules, Rust tests, existing real PG S049
book complaint/appeal, free comment report/appeal, paid comment
review/appeal and complete required CI, then post-merge. No deployment.
