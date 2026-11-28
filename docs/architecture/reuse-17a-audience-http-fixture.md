# REUSE-17A — shared audience HTTP test mechanics

H/I test-only slice; FLOW audience report acceptance. Base 003a39c10115f8f5baab8e5c4502ec5d560f5cc9.
Five independent existing targets retain their test functions, fixtures,
legacy database proof, SQL execution (including stdin for group returns),
lock tables, local privacy checks (including CSP where present), routes,
cookie values, expected statuses, and all assertions.

Only three byte-identical functions are extracted: psql client construction,
HTTP GET with the provided cookie, and UTF-8 response body capped at 256 KiB.
The original client guard and all three PGOPTIONS remain unchanged; the
existing legacy_analytics::url() verifies the actual isolated database.
Do not use the fixture with current or production data.

The three bodies total 32 lines; removing four extra copies removes 128
repeated function lines before wiring. All five targets keep their existing
PG18 acceptance commands; the support file is mapped to rust and
reading_analytics and a selector regression proves this. No new standalone
test target, migration, production code, SQL, or deployment. Exact-head and
post-merge acceptance are pending.
