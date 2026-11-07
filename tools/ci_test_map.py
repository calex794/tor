"""I-07: existing CI suites and their Cargo entry points (no test replacement)."""

# A suite consumes its roots plus their transitive local Cargo dependencies.
SUITES = {
    "rust": ("rust-demo.yml", ("*",)),
    "postgres": ("tor-pg-runtime.yml", ("*",)),
    "migrations": ("postgres-migrations.yml", ("tor-migrations",)),
    "reader_browser": ("tor-reader-visual.yml", ("tor-web", "tor-migrations", "tor-observability")),
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
MANUAL_WORKFLOWS = {"tor-vps-staging.yml", "tor-one-click-release.yml", "chapter-import-s3.yml"}
# Safe local-only performance workflows run for changes to their harness, and
# can also be started manually. They never contact live infrastructure.
PERFORMANCE_WORKFLOWS = {"tor-http-load.yml"}
# Dedicated throwaway network-namespace ingress test: no PostgreSQL, no live hosts.
ISOLATED_GATEWAY_WORKFLOWS = {"tor-vps-ingress.yml"}
# Additional exact-head PG/HTTP/Chromium acceptance; never a deployment gate.
ACCEPTANCE_WORKFLOWS = {"rec-feedback-acceptance.yml"}
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
    "tor-route_intelligence", "tor-support", "tor-referrals", "tor-social",
}
KNOWN_PACKAGES = CRITICAL_PACKAGES | {
    "tor-web", "tor-search", "tor-recommendations", "tor-observability",
    "tor-discussions",
}

# Standalone test/harness edits do not change the library consumed by other suites.
# New/unclassified tests deliberately fall back to full, never to an empty pack.
TEST_SUITES = {
    "tools/rec13_browser_acceptance.sh": {"postgres"},
    "tools/rec13_browser.cjs": {"postgres"},
    "crates/recommendations/tests/feedback_pg.rs": {"rust", "postgres"},
    "crates/recommendations/tests/generations_pg.rs": {"rust", "postgres"},
    "crates/web/tests/recommendation_feedback_http_pg.rs": {"rust", "postgres"},
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
    "crates/search/tests/f04_rising_perf_pg.rs": {"rust", "search_load"},
    "crates/search/tests/search_pg.rs": {"rust", "postgres"},
    "crates/search/tests/catalog_window_pg.rs": {"rust", "postgres"},
    "crates/web/tests/catalog_pagination_http_pg.rs": {"rust", "postgres"},
    "crates/reading/tests/book_likes_pg.rs": {"rust", "postgres"},
    "crates/web/tests/book_likes_http_pg.rs": {"rust", "postgres"},
    "crates/search/tests/perf233_projection_pg.rs": {"rust", "postgres"},
    "crates/search/tests/f03_unicode_pg.rs": {"rust", "postgres"},
    "crates/referrals/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/social/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/support/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/web/tests/support_http_pg.rs": {"rust", "postgres"},
    "crates/accounts/tests/referral_signup_decision_pg.rs": {"rust", "postgres"},
    "crates/recommendations/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/discussions/tests/slices_pg.rs": {"rust", "postgres"},
    "crates/discussions/tests/public_book_comments_pg.rs": {"rust", "postgres"},
    "crates/observability/tests/store_sqlite.rs": {"rust"},
    "crates/observability/tests/trace_store_daemon.rs": {"rust"},
    "tools/visual-smoke.cjs": {"visual"},
    "tools/operator_trace_visual_smoke.cjs": {"visual"},
    "tools/operator_trace_pg_smoke.py": {"reader_browser"},
    "tools/operator_trace_pg_browser.cjs": {"reader_browser"},
    "tools/newboo_cover_visual_smoke.cjs": {"visual"},
    "tools/reader_visual_smoke.cjs": {"reader_browser"},
    "tools/mail_browser_pg.cjs": {"reader_browser"},
    "tools/support_browser_pg.cjs": {"reader_browser"},
    "tools/referral_share_browser_smoke.cjs": {"reader_browser"},
    "tools/author_chapter_visual_smoke.cjs": {"reader_browser"},
    "tools/loop_01_browser_smoke.cjs": {"reader_browser"},
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
    "tools/s061_cutover_pg_smoke.py": {"postgres"},
    "tools/tor_dr_archive_crypto.cjs": {"backup_recovery", "purchased_recovery"},
    "tools/tor_dr_s3_backup_smoke.sh": {"backup_recovery"},
    "tools/tor_s334_paid_restore_smoke.sh": {"purchased_recovery"},
}

# Explicit PG integration test targets currently executed by the mandatory
# postgres-runtime job (and the full Rust workspace baseline). A test-only PR
# must run both suites, not all fourteen infrastructure/browser suites.
# NEVER add production src or unknown tests here; test_ci_workflow_wiring.py
# verifies every target is directly invoked in both PG workflows and no other
# specialized job. Changes to this authority map itself always use full CI.
POSTGRES_RUNTIME_ONLY_TEST_FILES = frozenset({
    "crates/migrations/tests/e08_installed_upgrade_pg.rs",
    "crates/traffic_behavior/tests/ril_rollups_pg.rs",
    "crates/traffic_behavior/tests/ril_costs_pg.rs",
    "crates/traffic_behavior/tests/ril_sql_costs_pg.rs",
    "crates/web/tests/ril_sqlx_http_pg.rs",
    "crates/web/tests/f04_rising_pg.rs",
    "crates/storage/tests/keyring_rotation_pg.rs",
    "crates/storage/tests/storage_gc_pg.rs",
    "crates/storage/tests/storage_paid_grant_pg.rs",
    "crates/storage/tests/storage_paid_asset_pg.rs",
    "crates/web/tests/cover_worker_pg.rs",
    "crates/books/tests/postgres_catalog.rs",
    "crates/books/tests/shadow_age_visibility_pg.rs",
    "crates/search/tests/actor_catalog_age_pg.rs",
    "crates/search/tests/seo_pg.rs",
    "crates/books/tests/authoring_pg.rs",
    "crates/books/tests/book_lifecycle_pg.rs",
    "crates/books/tests/chapter_file_import_pg.rs",
    "crates/books/tests/book_members_pg.rs",
    "crates/books/tests/book_related_pg.rs",
    "crates/web/tests/public_related_http_pg.rs",
    "crates/books/tests/title_suggestions_pg.rs",
    "crates/books/tests/revenue_shares_pg.rs",
    "crates/web/tests/s020_age_editor_http_pg.rs",
    "crates/web/tests/s020_age_editor_browser_pg.rs",
    "crates/books/tests/seo_description_pg.rs",
    "crates/books/tests/free_immediate_publish_pg.rs",
    "crates/books/tests/free_published_edit_pg.rs",
    "crates/books/tests/chapter_release_pg.rs",
    "crates/books/tests/scheduled_release_pg.rs",
    "crates/books/tests/book_delayed_schedule_pg.rs",
    "crates/books/tests/book_slot_occurrences_pg.rs",
    "crates/books/tests/book_slot_frontier_pg.rs",
    "crates/books/tests/book_slot_release_pg.rs",
    "crates/books/tests/book_slot_paid_release_pg.rs",
    "crates/web/tests/scheduled_release_http_pg.rs",
    "crates/books/tests/paid_storage_pg.rs",
    "crates/books/tests/paid_published_edit_pg.rs",
    "crates/books/tests/paid_assets_pg.rs",
    "crates/reading/tests/postgres_reading.rs",
    "crates/reading/tests/age_tier_reader_pg.rs",
    "crates/reading/tests/seo_public_book_pg.rs",
    "crates/reading/tests/progress_postgres.rs",
    "crates/reading/tests/age_tier_progress_pg.rs",
    "crates/reading/tests/slices_pg.rs",
    "crates/reading/tests/completion_pg.rs",
    "crates/reading/tests/chapter_thanks_pg.rs",
    "crates/reading/tests/book_stats_pg.rs",
    "crates/reading/tests/continuation_demand_pg.rs",
    "crates/reading/tests/follow_updates_pg.rs",
    "crates/web/tests/continuation_demand_http_pg.rs",
    "crates/reading/tests/paid_completion_pg.rs",
    "crates/reading/tests/paid_tier_entitlement_pg.rs",
    "crates/reading/tests/library_postgres.rs",
    "crates/reading/tests/library_age_pg.rs",
    "crates/reading/tests/library_saved_free_pg.rs",
    "crates/web/tests/c08_saved_free_http_pg.rs",
    "crates/web/tests/c08_pending_library_http_pg.rs",
    "crates/web/tests/book_follow_http_pg.rs",
    "crates/web/tests/chapter_file_import_http_pg.rs",
    "crates/web/tests/auth_postgres.rs",
    "crates/recommendations/tests/book_stars_pg.rs",
    "crates/recommendations/tests/completion_consumer_pg.rs",
    "crates/recommendations/tests/achievements_pg.rs",
    "crates/discussions/tests/text_issues_pg.rs",
    "crates/commerce/tests/slices_pg.rs",
    "crates/commerce/tests/batch_checkout_pg.rs",
    "crates/commerce/tests/reader_follow_links_pg.rs",
    "crates/commerce/tests/layers_pg.rs",
    "crates/web/tests/layer_picker_http_pg.rs",
    "crates/web/tests/layer_checkout_http_pg.rs",
    "crates/web/tests/layer_storefront_http_pg.rs",
    "crates/commerce/tests/direct_swap_pg.rs",
    "crates/commerce/tests/d08_finance_pg.rs",
    "crates/web/tests/postgres_http.rs",
    "crates/web/tests/direct_swap_browser_http_pg.rs",
    "crates/web/tests/direct_refund_browser_http_pg.rs",
    "crates/web/tests/free_save_browser_pg.rs",
    "crates/web/tests/book_stars_http_pg.rs",
    "crates/web/tests/chapter_thanks_http_pg.rs",
    "crates/web/tests/book_stats_http_pg.rs",
    "crates/web/tests/book_comments_http_pg.rs",
    "crates/web/tests/paid_grants_http_pg.rs",
    "crates/web/tests/ril_behavior_consent_pg.rs",
    "crates/web/tests/password_change_http_pg.rs",
    "crates/social/tests/chapter_release_notifications_pg.rs",
    "crates/web/tests/mail_http_pg.rs",
    "crates/accounts/tests/access_audit_pg.rs",
    "crates/accounts/tests/auth_observation_pg.rs",
    "crates/accounts/tests/credentials_pg.rs",
    "crates/accounts/tests/slices_pg.rs",
    "crates/web/tests/moderation_books_pg.rs",
    "crates/web/tests/account_settings_http_pg.rs",
    "crates/reading/tests/notification_preferences_pg.rs",
    "crates/web/tests/account_access_audit_pg.rs",
    "crates/web/tests/account_sessions_http_pg.rs",
})
TEST_SUITES.update({path: {"rust", "postgres"} for path in POSTGRES_RUNTIME_ONLY_TEST_FILES})

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
