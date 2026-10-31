"""RIL-08 CI-negative tests: guard removal must break a check, not just docs."""
from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_governance import check  # noqa: E402

INVENTORY = "docs/contracts/ril-route-guards-v1.json"


class RilGovernanceTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        for src in (ROOT / "crates").glob("*/src/**/*.rs"):
            self.copy(src)
        self.copy(ROOT / INVENTORY)

    def copy(self, src: Path):
        target = self.root / src.relative_to(ROOT)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, target)

    def mutate(self, path: str, before: str, after: str, start: str = ""):
        target = self.root / path
        source = target.read_text(encoding="utf-8")
        if start:
            begin = source.index(start)
            prefix = source[:begin]
            source = source[begin:]
        else:
            prefix = ""
        self.assertIn(before, source)
        target.write_text(prefix + source.replace(before, after, 1), encoding="utf-8")

    def assert_detected(self, fragment: str):
        findings = check(self.root)
        self.assertTrue(any(fragment in finding for finding in findings), findings)

    def test_current_registration_and_guards_are_consistent(self):
        self.assertEqual([], check(self.root))

    def test_unknown_route_requires_explicit_security_review(self):
        self.mutate(
            "crates/web/src/lib.rs",
            '.route("/catalog", get(catalog_http::catalog))',
            '.route("/catalog", get(catalog_http::catalog)).route("/internal/export", get(health))',
        )
        self.assert_detected("route without reviewed security policy")

    def test_swapped_handler_is_not_authorized_by_old_policy(self):
        self.mutate(
            "crates/web/src/auth.rs",
            '.route("/me/books/{id}", get(author_book_page::page))',
            '.route("/me/books/{id}", get(new_author_book))',
        )
        self.assert_detected("route without reviewed security policy")

    def test_session_guard_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/author_book_page.rs",
            "session_user(&state, &headers)",
            "unverified_user(&state, &headers)",
            start="pub(super) async fn page(",
        )
        self.assert_detected("missing verified PG session guard")

    def test_owner_guard_removal_fails(self):
        self.mutate(
            "crates/web/src/author_chapter_http.rs",
            "release_book(actor, id)",
            "release_global(id)",
            start="async fn publish_book(",
        )
        self.assert_detected("missing actor-bound owner repository call")

    def test_seo_description_owner_guard_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/book_description.rs",
            "update_book_description(actor, book_id, &form.description)",
            "update_book_description(book_id, &form.description)",
            start="pub(super) async fn save(",
        )
        self.assert_detected("missing actor-bound owner repository call")

    def test_seo_description_csrf_bypass_fails(self):
        self.mutate(
            "crates/web/src/auth/book_description.rs",
            "state.cookies.same_origin(&headers)",
            "state.cookies.same_origin_skip(&headers)",
            start="pub(super) async fn save(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_scope_bound_repository_guard_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/sync_actions.rs",
            "purchased_state(actor, purchase_id, chapter_id, version_id)",
            "purchased_state_global(purchase_id, chapter_id, version_id)",
            start="pub(super) async fn get_purchased(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_reader_saved_state_remains_actor_scoped(self):
        self.mutate(
            "crates/web/src/auth/private_reader.rs",
            "saved_state(actor, chapter_id, version_id)",
            "saved_state(chapter_id, version_id)",
            start="pub(super) async fn page(",
        )
        # Both reading-state calls must be actor-scoped; weakening just one
        # must never be interpreted as a successful policy test.
        self.mutate(
            "crates/web/src/auth/private_reader.rs",
            "free_state(actor, chapter_id)",
            "free_state(chapter_id)",
            start="pub(super) async fn page(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_continuation_demand_cannot_ignore_actor_scope(self):
        self.mutate(
            "crates/web/src/auth/continuation_demand.rs",
            "record_verified_continuation(actor, book, chapter_id)",
            "record_verified_continuation(book, chapter_id)",
            start="pub(super) async fn submit(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_complete_chapter_never_loses_authenticated_actor(self):
        self.mutate(
            "crates/web/src/auth/complete_chapter.rs",
            ".save_free(actor, chapter_id, version_id, place, form.expected_revision)",
            ".save_free_unscoped(chapter_id, version_id, place, form.expected_revision)",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_csrf_rejection_removal_fails(self):
        self.mutate(
            "crates/web/src/auth.rs",
            "state.cookies.same_origin(&headers)",
            "state.cookies.same_origin_bypass(&headers)",
            start="async fn save_place(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_shared_library_helper_csrf_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/library_actions.rs",
            "state.cookies.same_origin(headers)",
            "state.cookies.same_origin_bypass(headers)",
            start="async fn write(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_ranking_csrf_helper_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/rankings.rs",
            "state.cookies.same_origin(headers)",
            "state.cookies.bypass_origin(headers)",
            start="fn csrf_valid(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_ranking_actor_guard_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/rankings.rs",
            "actor(&state, &headers).await",
            "unverified_actor(&state, &headers).await",
            start="pub(super) async fn api_state(",
        )
        self.assert_detected("missing verified PG session guard")

    def test_distributed_rate_limiter_removal_fails(self):
        self.mutate(
            "crates/web/src/auth/login_limiter.rs",
            "permit_auth_attempt(",
            "unlimited_auth_attempt(",
        )
        self.assert_detected("missing distributed authentication rate limit")

    def test_policy_cannot_downgrade_private_route_to_public(self):
        path = self.root / INVENTORY
        data = json.loads(path.read_text(encoding="utf-8"))
        item = next(x for x in data["routes"]
                    if x["template"] == "/me" and x["method"] == "GET")
        item["policy"] = "public"
        path.write_text(json.dumps(data), encoding="utf-8")
        self.assert_detected("protected route downgraded to public/entry")

    def test_duplicate_reviewed_binding_fails(self):
        path = self.root / INVENTORY
        data = json.loads(path.read_text(encoding="utf-8"))
        data["routes"].append(dict(data["routes"][0]))
        path.write_text(json.dumps(data), encoding="utf-8")
        self.assert_detected("duplicate route-policy binding")


    def test_follow_write_requires_actor_bound_repository_call(self):
        self.mutate(
            "crates/web/src/auth/book_follow_actions.rs",
            "state.book_follows.follow(actor, book_id)",
            "state.book_follows.follow(book_id)",
            start="async fn write(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_unfollow_write_requires_actor_bound_repository_call(self):
        self.mutate(
            "crates/web/src/auth/book_follow_actions.rs",
            "state.book_follows.unfollow(actor, book_id)",
            "state.book_follows.unfollow(book_id)",
            start="async fn write(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_follow_write_requires_origin_and_csrf(self):
        self.mutate(
            "crates/web/src/auth/book_follow_actions.rs",
            "state.cookies.same_origin(headers)",
            "state.cookies.same_origin_bypass(headers)",
            start="async fn write(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_release_read_state_requires_actor_bound_repository_call(self):
        self.mutate(
            "crates/web/src/auth/follow_read_state.rs",
            "state.follow_updates.set_read_state(actor, id, read)",
            "state.follow_updates.set_read_state(id, read)",
            start="pub(super) async fn submit(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_release_read_state_requires_origin_and_csrf(self):
        self.mutate(
            "crates/web/src/auth/follow_read_state.rs",
            "state.cookies.same_origin(&headers)",
            "state.cookies.unchecked_origin(&headers)",
            start="pub(super) async fn submit(",
        )
        self.assert_detected("missing Origin+CSRF rejection")


    def test_reset_request_cannot_lose_csrf_guard(self):
        self.mutate("crates/web/src/auth/password_reset.rs", ".valid_csrf(", ".ignored_csrf(", start="pub(super) async fn request(")
        self.assert_detected("missing Origin+CSRF rejection")

    def test_reset_confirmation_cannot_lose_rate_limit(self):
        self.mutate("crates/web/src/auth/password_reset.rs", "login_limiter::limit(", "removed_limiter(", start="pub(super) async fn confirm(")
        self.assert_detected("missing distributed recovery rate limit")

    def test_recovery_exception_is_restricted_to_two_reviewed_routes(self):
        target = self.root / INVENTORY
        data = json.loads(target.read_text())
        row = next(r for r in data["routes"] if r["template"] == "/api/v1/me" and r["method"] == "PATCH")
        row["policy"] = "recovery"
        target.write_text(json.dumps(data))
        self.assert_detected("recovery exception is not allowed here")
    def test_delivery_preferences_save_is_actor_scoped(self):
        self.mutate(
            "crates/web/src/auth/notification_settings.rs",
            ".set_delivery_preference(actor, mode)",
            ".set_delivery_preference(mode)",
            start="pub(super) async fn save(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_delivery_preferences_save_requires_origin_and_csrf(self):
        self.mutate(
            "crates/web/src/auth/notification_settings.rs",
            "state.cookies.same_origin(&headers)",
            "state.cookies.unchecked_origin(&headers)",
            start="pub(super) async fn save(",
        )
        self.assert_detected("missing Origin+CSRF rejection")


    def test_mail_block_cannot_lose_the_authenticated_actor(self):
        self.mutate(
            "crates/web/src/auth/mail.rs",
            ".set_block(actor, page.peer_id, form.blocked)",
            ".set_block(page.peer_id, form.blocked)",
            start="pub(super) async fn block(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_mail_csrf_guard_removal_is_detected(self):
        self.mutate(
            "crates/web/src/auth/mail.rs",
            "state.cookies.valid_csrf(headers, t)",
            "state.cookies.valid_csrf_skip(headers, t)",
            start="fn csrf(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_notification_feed_requires_verified_actor_scope(self):
        self.mutate(
            "crates/web/src/auth/notifications.rs",
            "store.list_section(actor, query.before, 50, tab)",
            "store.list_section(1, query.before, 50, tab)",
            start="pub(super) async fn page(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_notification_tabs_require_section_filtered_repository(self):
        self.mutate(
            "crates/web/src/auth/notifications.rs",
            "store.list_section(actor, query.before, 50, tab)",
            "store.list(actor, query.before, 50)",
            start="pub(super) async fn page(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_notification_preferences_save_is_actor_scoped(self):
        self.mutate(
            "crates/web/src/auth/notifications.rs",
            ".set_preferences(actor, &enabled_kinds)",
            ".set_preferences(1, &enabled_kinds)",
            start="pub(super) async fn save_preferences(",
        )
        self.assert_detected("missing actor-bound scoped repository call")

    def test_notification_preferences_save_requires_csrf(self):
        self.mutate(
            "crates/web/src/auth/notifications.rs",
            "if let Err(status) = csrf(",
            "if let Err(status) = unchecked_csrf(",
            start="pub(super) async fn save_preferences(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

    def test_notification_read_requires_origin_and_csrf(self):
        self.mutate(
            "crates/web/src/auth/notifications.rs",
            "state.cookies.same_origin(headers)",
            "state.cookies.unchecked_origin(headers)",
            start="fn csrf(",
        )
        self.assert_detected("missing Origin+CSRF rejection")

if __name__ == "__main__":
    unittest.main()
