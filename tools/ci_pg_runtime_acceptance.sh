#!/usr/bin/env bash
# Shared PostgreSQL runtime acceptance. Preserve the original shell-local
# Cargo instrumentation and per-command feature flags in all three shards.
set -e
case "${CI_PG_SHARD:-}" in
  foundation|reading|commerce) ;;
  *) echo 'Unsupported PostgreSQL CI shard' >&2; exit 2 ;;
esac
# REC13 Chromium runs in its own required PostgreSQL job; remaining commands keep their order.
ci_pg_started=$SECONDS
ci_pg_phase_done() {
  local phase="$1"
  echo "CI_PG_PHASE_SECONDS phase=$phase seconds=$((SECONDS-ci_pg_started))"
  ci_pg_started=$SECONDS
}
# Diagnose compilation versus test execution without changing any Cargo command.
# The env -u executions retain their own timing boundaries.
ci_pg_cargo_index=0
cargo() {
  local start_us=${EPOCHREALTIME/./}
  local status=0
  local package=unknown target=lib prev= arg
  for arg in "$@"; do
    case "$prev" in
      -p) package="$arg" ;;
      --test) target="$arg" ;;
    esac
    case "$arg" in
      -p|--test) prev="$arg" ;;
      *) prev= ;;
    esac
  done
  command cargo "$@" || status=$?
  local end_us=${EPOCHREALTIME/./}
  ci_pg_cargo_index=$((ci_pg_cargo_index + 1))
  printf 'CI_PG_CARGO_MS seq=%d package=%s target=%s elapsed_ms=%d status=%d\n' \
    "$ci_pg_cargo_index" "$package" "$target" "$(((end_us-start_us)/1000))" "$status"
  return "$status"
}
if [[ "$CI_PG_SHARD" == "foundation" ]]; then
  cargo test -p tor-traffic_behavior --test ril_rollups_pg -- --ignored --test-threads=1
  cargo test -p tor-traffic_behavior --test ril_costs_pg -- --ignored --test-threads=1
  cargo test -p tor-traffic_behavior --test ril_sql_costs_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test ril_sqlx_http_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test storage_registry_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test paid_author_source_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test free_chapter_storage_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test keyring_rotation_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test storage_gc_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test storage_reconciliation_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test storage_paid_grant_pg -- --ignored --test-threads=1
  cargo test -p tor-storage --features pg --test storage_paid_asset_pg -- --ignored --test-threads=1
  cargo test -p tor-ingest --test cover_jobs_pg -- --ignored
  cargo test -p tor-ingest --test outbox_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test admin_jobs_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test cover_worker_pg -- --ignored
  cargo test -p tor-books --test postgres_catalog -- --ignored
  cargo test -p tor-books --test shadow_age_visibility_pg -- --ignored --test-threads=1
  ci_pg_phase_done "traffic_storage_catalog"
fi
if [[ "$CI_PG_SHARD" == "foundation" ]]; then
  cargo test -p tor-search --test search_pg -- --ignored
  cargo test -p tor-search --test f04_rising_pg -- --ignored --test-threads=1
  cargo test -p tor-search --test catalog_window_pg -- --ignored --test-threads=1
  cargo test -p tor-search --test actor_catalog_age_pg -- --ignored --test-threads=1
  cargo test -p tor-search --test perf233_projection_pg -- --ignored --test-threads=1
  cargo test -p tor-search --test f03_unicode_pg -- --ignored
  cargo test -p tor-books --test authoring_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test book_lifecycle_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test origin_metadata_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test chapter_file_import_pg -- --ignored --test-threads=1
  cargo test -p tor-imports --test chapter_import_jobs_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test chapter_import_job_finalize_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test chapter_import_queue_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test chapter_import_worker_contention_pg -- --ignored --test-threads=1 --show-output
  cargo test -p tor-books --test book_members_pg b_09_ -- --ignored --test-threads=1
  cargo test -p tor-books --test book_related_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test public_related_http_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test title_suggestions_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test revenue_shares_pg b_10_ -- --ignored --test-threads=1
  cargo test -p tor-books --lib b_09_pg_owner_swap -- --ignored --test-threads=1
  cargo test -p tor-accounts --lib a_b09_pg_user_tier -- --ignored --test-threads=1
  cargo test -p tor-books --lib b_09_pg_book_tier -- --ignored --test-threads=1
  cargo test -p tor-books --lib s020_pg_access_level_monotonic_live_grant_membership_and_cas -- --ignored --test-threads=1
  cargo test -p tor-web --test s020_age_editor_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s020_age_editor_browser_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test seo_description_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test free_immediate_publish_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test free_s3_lifecycle_pg -- --ignored --test-threads=1
   cargo test -p tor-books --test d001_logical_retention_pg -- --ignored --test-threads=1
   cargo test -p tor-books --test d001_historical_anchor_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test free_published_edit_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test chapter_release_pg -- --ignored
  cargo test -p tor-books --test scheduled_release_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test book_delayed_schedule_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test book_slot_occurrences_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test book_slot_frontier_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test book_slot_release_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test book_slot_paid_release_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test scheduled_release_http_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test paid_storage_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test paid_published_edit_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test paid_assets_pg -- --ignored --test-threads=1
  ci_pg_phase_done "books_import_authoring"
fi
if [[ "$CI_PG_SHARD" == "reading" ]]; then
  cargo test -p tor-reading --test postgres_reading -- --ignored
  cargo test -p tor-reading --test age_tier_reader_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test seo_public_book_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test progress_postgres -- --ignored
  cargo test -p tor-reading --test ret00_resume_next_free_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test age_tier_progress_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test slices_pg c_09_ -- --ignored --test-threads=1
  cargo test -p tor-reading --test completion_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test chapter_thanks_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test book_stats_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test book_likes_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test continuation_demand_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test follow_updates_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test continuation_demand_http_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test paid_completion_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test paid_tier_entitlement_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test slices_pg c_10_ -- --ignored --test-threads=1
  cargo test -p tor-reading --test library_postgres -- --ignored
  cargo test -p tor-reading --test library_age_pg -- --ignored --test-threads=1
  cargo test -p tor-reading --test library_saved_free_pg -- --ignored --nocapture --test-threads=1
  cargo test -p tor-reading --test c08_verified_free_open_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test c08_saved_free_http_pg -- --ignored --test-threads=1
  cargo test -p tor-books --test ordinary_deletion_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test c08_pending_library_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test book_follow_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test ret01_import_follow_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test chapter_file_import_http_pg -- --ignored --test-threads=1 --show-output
  ci_pg_phase_done "reading_library"
fi
if [[ "$CI_PG_SHARD" == "reading" ]]; then
  cargo test -p tor-referrals --test slices_pg f_09_pg_ -- --ignored --test-threads=1
  cargo test -p tor-accounts --test referral_signup_decision_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test auth_postgres f_09_http_ -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test slices_pg f_05_ -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test feedback_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test recommendation_preferences_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test recommendation_preferences_legacy_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test catalog_feed_pg -- --ignored --test-threads=1 --skip contextual_scale::model_page_is_bounded_with_100k_books_and_reader_requests
  cargo test -p tor-recommendations --test catalog_legacy_history_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test analytics_invalidation_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test generations_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test recommendation_feedback_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test recommendation_preferences_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test audience_retirement_http_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test book_stars_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test completion_consumer_pg -- --ignored --test-threads=1
  cargo test -p tor-recommendations --test achievements_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s046_achievements_http_pg -- --ignored --test-threads=1
  cargo test -p tor-social --test slices_pg e_05_ -- --ignored --test-threads=1
  cargo test -p tor-discussions --test slices_pg e_01_ -- --ignored --test-threads=1
  cargo test -p tor-discussions --test public_book_comments_pg e_02_ -- --ignored --test-threads=1
  cargo test -p tor-discussions --test paid_chapter_comments_pg s011_ -- --ignored --test-threads=1
  cargo test -p tor-discussions --test text_issues_pg e_12_ -- --ignored --test-threads=1
  ci_pg_phase_done "referrals_recommendations_social"
fi
if [[ "$CI_PG_SHARD" == "commerce" ]]; then
  cargo test -p tor-commerce --test slices_pg d_02_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test batch_checkout_pg d_s035_ -- --ignored --test-threads=1
  # S020 P0: real authenticated single/batch HTTP checkout age parity.
  TOR_PAID_CHECKOUT_ENABLED=1 cargo test -p tor-web --test s020_age_checkout_http_pg s020_age_checkout_http_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test slices_pg d_03_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test reader_follow_links_pg -- --ignored --test-threads=1
  cargo test -p tor-commerce --test slices_pg b_10_ -- --ignored --test-threads=1
   cargo test -p tor-commerce --test slices_pg d_06_ -- --ignored --test-threads=1 --show-output
  cargo test -p tor-commerce --test layers_pg d_05_ -- --ignored --test-threads=1
  # H/UI-18: execute new package-cover ownership, moderation, cleanup and entitlement PG test.
  cargo test -p tor-commerce --test layers_pg h_ui_18_ -- --ignored --test-threads=1
  # H/UI-19: enforce new direct and batch sale restrictions on disposable PostgreSQL.
  cargo test -p tor-commerce --test layers_pg h_ui_19_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test batch_checkout_pg h_ui_19_ -- --ignored --test-threads=1
  # H/UI-21: registration-based owner-selected early window, with atomic mixed batch.
  cargo test -p tor-commerce --test layers_pg h_ui_21_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test batch_checkout_pg h_ui_21_ -- --ignored --test-threads=1
  # H/UI-20: validate only visible active package links and no direct debit.
  cargo test -p tor-commerce --test layers_pg h_ui_20_ -- --ignored --test-threads=1
  TOR_PAID_CHECKOUT_ENABLED=1 TOR_LAYER_PAGES_ENABLED=1 TOR_LAYER_CHECKOUT_ENABLED=1 cargo test -p tor-web --test package_paid_links_http_pg h_ui_20_ -- --ignored --test-threads=1
  # Enable owner Layer management for exactly this isolated PG/HTTP suite.
  # All other tests and the actual application retain the default OFF gate.
  TOR_LAYER_MANAGEMENT_ENABLED=1 cargo test -p tor-web --test layer_picker_http_pg d_05_ -- --ignored --test-threads=1
  # S031 Layer wallet + browser-form acceptance: explicit flags per disposable suite.
  TOR_PAID_CHECKOUT_ENABLED=1 TOR_LAYER_PAGES_ENABLED=1 TOR_LAYER_CHECKOUT_ENABLED=1 cargo test -p tor-web --test layer_checkout_http_pg d_05_ -- --ignored --test-threads=1
  TOR_PAID_CHECKOUT_ENABLED=1 TOR_LAYER_PAGES_ENABLED=1 TOR_LAYER_CHECKOUT_ENABLED=1 cargo test -p tor-web --test layer_storefront_http_pg d_05_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test direct_swap_pg d_06_swap_ -- --ignored --test-threads=1
   cargo test -p tor-commerce --test d08_finance_pg d_08_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test d08_finance_pg s038_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test d09_payout_reject_pg d_09_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test d09_payout_approval_quote_pg d_09_quote_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test d09_payout_posting_pg d_09_post_ -- --ignored --test-threads=1
  cargo test -p tor-commerce --test d09_payout_mature_pg d_09_post_ -- --ignored --test-threads=1
  cargo test -p tor-support --test slices_pg e_11_ -- --ignored --test-threads=1
  cargo test -p tor-mail --test slices_pg mail_01_ -- --ignored --test-threads=1
  cargo test -p tor-web --test support_http_pg e_11_ -- --ignored --test-threads=1
  ci_pg_phase_done "commerce_support"
fi
if [[ "$CI_PG_SHARD" == "commerce" ]]; then
  cargo test -p tor-web --test postgres_http -- --ignored
  cargo test -p tor-web --test catalog_pagination_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s033_personal_http_pg -- --ignored --test-threads=1
  cargo test -p tor-social --test book_shares_pg -- --ignored --test-threads=1
  cargo test -p tor-social --test book_collections_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s016_book_shares_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s049_book_share_reports_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s049_book_content_reports_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s049_comment_appeals_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s049_free_comment_reports_http_pg -- --ignored --nocapture --test-threads=1
  cargo test -p tor-web --test s009_collections_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test s030_owner_transfer_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test auth_postgres -- --ignored
  env -u TOR_DIRECT_SWAP_BROWSER_ENABLED -u TOR_PAID_CHECKOUT_ENABLED cargo test -p tor-web --test direct_swap_browser_http_pg d_06_swap_http_disabled_by_default -- --ignored --test-threads=1
  TOR_PAID_CHECKOUT_ENABLED=1 TOR_DIRECT_SWAP_BROWSER_ENABLED=1 cargo test -p tor-web --test direct_swap_browser_http_pg d_06_swap_http_preview_csrf_actor_stale_quote_commit_and_replay -- --ignored --test-threads=1
  env -u TOR_PAID_CHECKOUT_ENABLED -u TOR_ADMIN_SUPPORT_REFUNDS_ENABLED cargo test -p tor-web --test direct_refund_browser_http_pg s036_refunds_not_routed_to_buyer -- --ignored --test-threads=1
  TOR_PAID_CHECKOUT_ENABLED=1 TOR_ADMIN_SUPPORT_REFUNDS_ENABLED=1 cargo test -p tor-web --test direct_refund_browser_http_pg s036_admin_support_search_exact_purchase_prepare_approve_and_replay -- --ignored --test-threads=1
  
  env -u TOR_S039_PAYOUT_REJECT_HTTP_ENABLED cargo test -p tor-web --test s039_payout_admin_http_pg s039_http_disabled_by_default -- --ignored --test-threads=1
  TOR_S039_PAYOUT_REJECT_HTTP_ENABLED=1 cargo test -p tor-web --test s039_payout_admin_http_pg s039_http_admin_role_csrf_idempotent_rejection -- --ignored --test-threads=1
  env -u TOR_S039_PAYOUT_QUOTE_HTTP_ENABLED cargo test -p tor-web --test s039_payout_quote_http_pg s039_quote_http_disabled_by_default -- --ignored --test-threads=1
  TOR_S039_PAYOUT_QUOTE_HTTP_ENABLED=1 cargo test -p tor-web --test s039_payout_quote_http_pg s039_quote_http_admin_only_and_no_wallet_movement -- --ignored --test-threads=1
  cargo test -p tor-web --test s039_payout_step_up_pg s039_password_step_up_is_session_bound_and_posted_only_once -- --ignored --test-threads=1
  env -u TOR_S039_PAYOUT_POST_HTTP_ENABLED cargo test -p tor-web --test s039_payout_post_http_pg s039_payout_post_http_closed_by_default -- --ignored --test-threads=1
  TOR_S039_PAYOUT_POST_HTTP_ENABLED=1 cargo test -p tor-web --test s039_payout_post_http_pg s039_payout_post_http_role_password_csrf_exact_once_and_replay -- --ignored --test-threads=1
  cargo test -p tor-web --test free_save_browser_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test book_stars_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test chapter_thanks_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test book_stats_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test book_likes_http_pg -- --ignored --test-threads=1
  cargo test -p tor-web --test book_comments_http_pg e_02_ -- --ignored --test-threads=1
  cargo test -p tor-web --test paid_grants_http_pg -- --ignored --nocapture --test-threads=1
  TOR_RIL_SINGLE_INSTANCE=1 TOR_FLOW_PUBLIC_OPT_IN_ENABLED=1 \
    TOR_RIL_HMAC_KEY="$(printf '4f%.0s' {1..32})" \
    cargo test -p tor-web --test ril_behavior_consent_pg -- --ignored --test-threads=1
  ci_pg_phase_done "http_security"
fi
