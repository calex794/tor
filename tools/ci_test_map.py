"""I-07: existing CI suites and their Cargo entry points (no test replacement)."""

# A suite consumes its roots plus their transitive local Cargo dependencies.
SUITES = {
    "rust": ("rust-demo.yml", ("*",)),
    "postgres": ("tor-pg-runtime.yml", ("*",)),
    "migrations": ("postgres-migrations.yml", ("tor-migrations",)),
    "reader_browser": ("tor-reader-visual.yml", ("tor-web", "tor-migrations")),
    "visual": ("visual-smoke.yml", ("tor-web", "tor-observability")),
    "cover_browser": ("tor-cover-browser.yml", ("tor-web", "tor-migrations", "tor-ingest")),
    "image": ("image-pipeline.yml", ("tor-ingest",)),
    "crypto": ("storage-crypto.yml", ("tor-storage", "tor-contracts")),
    "storage_s3": ("storage-s3.yml", ("tor-storage",)),
    "cover_s3": ("tor-cover-s3.yml", ("tor-web", "tor-ingest")),
    "search_load": ("tor-f02-load.yml", ("tor-search", "tor-migrations")),
    "paid_e2e": ("tor-paid-grant-e2e.yml", ("tor-web", "tor-migrations")),
    "backup_recovery": ("tor-disaster-recovery.yml", ("tor-web", "tor-migrations")),
    "purchased_recovery": ("tor-s334-restore.yml", ("tor-web", "tor-migrations")),
}

# One automatic CI workflow executes all suites; legacy workflow files remain manual-only.
CI_WORKFLOW = "tor-integration.yml"
CI_REQUIRED_JOB = "ci-required"
SUITE_JOB_IDS = {
    "rust": "demo",
    "postgres": "postgres-runtime",
    "migrations": "migrations",
    "reader_browser": "reader-browser",
    "visual": "visual",
    "cover_browser": "author-cover",
    "image": "image",
    "crypto": "crypto",
    "storage_s3": "s3-provider",
    "cover_s3": "s3-cover-build",
    "search_load": "f02-100k",
    "paid_e2e": "paid-private-e2e",
    "backup_recovery": "sealed-remote-copy",
    "purchased_recovery": "purchased-v1-recovery",
}

# Deployment workflows contact the VPS: operator dispatch only, never automatic CI.
MANUAL_WORKFLOWS = {"tor-vps-staging.yml", "tor-one-click-release.yml"}
# Safe local-only performance workflows run for changes to their harness, and
# can also be started manually. They never contact live infrastructure.
PERFORMANCE_WORKFLOWS = {"tor-http-load.yml"}
POSTGRES_SUITES = {
    "postgres", "migrations", "reader_browser", "cover_browser", "cover_s3",
    "search_load", "paid_e2e", "backup_recovery", "purchased_recovery",
}
BROWSER_SUITES = {"reader_browser", "visual", "cover_browser", "paid_e2e"}

# These production areas can change access, money, publication, or delivery.
# They must run the complete available B/C/D/G/H/I pack regardless of the graph.
CRITICAL_PACKAGES = {
    "tor-contracts", "tor-accounts", "tor-books", "tor-reading", "tor-commerce",
    "tor-storage", "tor-ingest", "tor-imports", "tor-migrations", "tor-traffic_behavior",
    "tor-route_intelligence", "tor-referrals", "tor-social",
}
KNOWN_PACKAGES = CRITICAL_PACKAGES | {
    "tor-web", "tor-search", "tor-recommendations", "tor-observability",
    "tor-discussions",
}

# Standalone test/harness edits do not change the library consumed by other suites.
# New/unclassified tests deliberately fall back to full, never to an empty pack.
TEST_SUITES = {
    "crates/ingest/tests/image_pipeline.rs": {"rust", "image"},
    "crates/ingest/tests/s3_provider.rs": {"rust", "cover_s3", "cover_browser"},
    "crates/ingest/tests/cover_jobs_pg.rs": {"rust", "postgres"},
    "crates/imports/tests/chapter_import_jobs_pg.rs": {"rust", "postgres"},
    "crates/books/tests/chapter_import_job_finalize_pg.rs": {"rust", "postgres"},
    "crates/web/tests/chapter_import_queue_http_pg.rs": {"rust", "postgres"},
    "crates/storage/tests/crypto_webcrypto.mjs": {"crypto"},
    "crates/storage/tests/private_objects.rs": {"rust", "crypto"},
    "crates/storage/tests/s3_private_provider.rs": {"rust", "storage_s3"},
    "crates/storage/tests/storage_registry_pg.rs": {"rust", "postgres"},
    "crates/contracts/tests/paid_delivery_contract.rs": {"rust", "crypto"},
    "crates/search/tests/f02_load_pg.rs": {"rust", "search_load"},
    "crates/search/tests/search_pg.rs": {"rust", "postgres"},
    "crates/search/tests/catalog_window_pg.rs": {"rust", "postgres"},
    "crates/web/tests/catalog_pagination_http_pg.rs": {"rust", "postgres"},
    "crates/search/tests/perf233_projection_pg.rs": {"rust", "postgres"},
    "crates/search/tests/f03_unicode_pg.rs": {"rust", "postgres"},
    "crates/referrals/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/social/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/accounts/tests/referral_signup_decision_pg.rs": {"rust", "postgres"},
    "crates/recommendations/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/discussions/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/discussions/tests/public_book_comments_pg.rs": {"rust", "postgres"},
    "crates/observability/tests/store_sqlite.rs": {"rust"},
    "crates/observability/tests/trace_store_daemon.rs": {"rust"},
    "tools/visual-smoke.cjs": {"visual"},
    "tools/operator_trace_visual_smoke.cjs": {"visual"},
    "tools/newboo_cover_visual_smoke.cjs": {"visual"},
    "tools/reader_visual_smoke.cjs": {"reader_browser"},
    "tools/referral_share_browser_smoke.cjs": {"reader_browser"},
    "tools/author_chapter_visual_smoke.cjs": {"reader_browser"},
    "tools/chapter_file_import_browser.cjs": {"reader_browser"},
    "tools/search_visual_smoke.cjs": {"reader_browser"},
    "tools/ranking_visual_smoke.cjs": {"reader_browser"},
    "tools/author_cover_pg_visual_smoke.cjs": {"cover_browser"},
    "tools/paid_reader_browser_smoke.cjs": {"reader_browser"},
    "tools/paid_reader_minio_chromium_e2e.cjs": {"paid_e2e"},
    "tools/paid_grant_webcrypto_e2e.mjs": {"paid_e2e", "purchased_recovery"},
    "tools/checkout_http_pg_smoke.py": {"postgres"},
    "tools/tor_pg_restart_smoke.sh": {"postgres"},
    "tools/tor_pg_backup_restore_smoke.sh": {"postgres"},
    "tools/tor_dr_archive_crypto.cjs": {"backup_recovery", "purchased_recovery"},
    "tools/tor_dr_s3_backup_smoke.sh": {"backup_recovery"},
    "tools/tor_s334_paid_restore_smoke.sh": {"purchased_recovery"},
}

# Only prose in these locations is cheap. Contracts/fixtures/configuration are not.
PROSE_FILES = {"README.md", "AGENTS.md", "docs/development.md"}
PROSE_PREFIXES = ("docs/research/", "docs/ci/")


# I-07/I-09: exact-path tools executed ONLY by Repository rules.
# Verified against all 18 CI workflows on 2026-10-29: CI metrics is a
# read-only Actions reporting script; its tests run in rules.yml architecture.
# Do not broaden to other tools/architecture tests. Every push to main still
# runs the complete 14-suite integration pack.
RULES_ONLY_FILES = frozenset({
    "tools/ci_metrics.py",
    "tests/architecture/test_ci_metrics.py",
})


def workflow_gate(suite):
    """A failed/missing planner runs the suite; cancellation can still stop it."""
    return (
        "${{ !cancelled() && (needs.select.result != 'success' || "
        f"fromJSON(needs.select.outputs.suites || '{{\"{suite}\":true}}').{suite}) "
        "}}"
    )
