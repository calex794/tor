"""S020 P0: cross-workstream acceptance coverage cannot silently disappear.

This architecture check verifies that executable PG/HTTP/Chromium/MinIO tests
and the server-side age gates remain wired into required CI. It does not replace
the tests themselves or authorize enabling the 18+/21+ editor in production.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class S020CrossSurfaceCoverage(unittest.TestCase):
    def read(self, path: str) -> str:
        return (ROOT / path).read_text(encoding="utf-8")

    def test_required_disposable_postgres_suites_are_in_both_workflows(self):
        # The former S020 handler was silently reverted along with its tests.
        # A future cross-line merge must not be green after deleting *either*.
        commands = (
            "cargo test -p tor-books --test shadow_age_visibility_pg",
            "cargo test -p tor-search --test actor_catalog_age_pg",
            "cargo test -p tor-books --test title_suggestions_pg",
            "cargo test -p tor-web --test public_related_http_pg",
            "cargo test -p tor-web --test s020_age_editor_http_pg",
            "cargo test -p tor-web --test s020_age_editor_browser_pg",
            "cargo test -p tor-recommendations --test slices_pg f_05_",
            "cargo test -p tor-recommendations --test book_stars_pg",
            "cargo test -p tor-web --test s016_book_shares_http_pg",
            "cargo test -p tor-web --test paid_grants_http_pg",
        )
        for workflow in (
            ".github/workflows/tor-integration.yml",
            ".github/workflows/tor-pg-runtime.yml",
        ):
            source = self.read(workflow)
            for command in commands:
                with self.subTest(workflow=workflow, command=command):
                    self.assertEqual(source.count(command), 1)

    def test_real_paid_encrypted_minio_and_live_revocation_remain_required(self):
        workflow = self.read(".github/workflows/tor-integration.yml")
        self.assertIn(
            "cargo test -p tor-web --features s3-paid --test paid_grants_s3_pg",
            workflow,
        )
        fixture = self.read("crates/web/tests/paid_grants_s3_pg.rs")
        self.assertIn("s020_paid_completion_age::verify(", fixture)
        self.assertIn("s020_paid_completion_age::verify_isolated_text_grants(", fixture)
        signed = self.read(
            "crates/web/tests/paid_grants_s3_pg/s020_paid_completion_age.rs"
        )
        for evidence in (
            "verify_isolated_text_grants",
            "assert_text_grant_denied",
            "assert_private_asset_grant",
            "StatusCode::FORBIDDEN",
            "StatusCode::TOO_MANY_REQUESTS",
        ):
            # 429 is not an acceptable denial: the isolated buyer protects
            # the production rate-limit budget for the main paid scenario.
            if evidence == "StatusCode::TOO_MANY_REQUESTS":
                self.assertIn("12/minute", signed)
            else:
                self.assertIn(evidence, signed)
        self.assertIn("active_version_id={v1}", signed)
        self.assertIn("access_level=NULL", signed)

    def test_one_book_age_upgrade_and_chromium_revoke_stay_executable(self):
        http = self.read("crates/web/tests/s020_age_editor_http_pg.rs")
        for route in (
            "/api/v1/books/",
            "/reader/",
            "/me/library/add/",
            "/api/v1/me/library",
            "/api/v1/me/rankings",
            "/books/",
        ):
            with self.subTest(route=route):
                self.assertIn(route, http)
        for token in (
            "s020_age_editor_is_off_by_default_and_monotonic_under_live_http",
            '"adult"',
            '"pro_adult"',
            "adult_enabled=TRUE",
            "is_active=FALSE",
        ):
            self.assertIn(token, http)
        browser = self.read("crates/web/tests/s020_age_editor_browser_pg.rs")
        self.assertIn(
            "s020_owner_age_editor_real_chromium_mobile_and_desktop", browser
        )
        js = self.read("tools/s020_age_editor_chromium.cjs")
        self.assertIn("chromium.launch", js)
        self.assertIn("390", js)

    def test_live_sql_gates_and_admin_independence_stay_in_source(self):
        # These probes are intentionally broad: refactoring SQL is allowed,
        # silently dropping an entire gate is not.
        paths_and_tokens = {
            "crates/reading/src/repositories/postgres.rs": (
                "public_book_page_for_session",
                "public_chapter_for_session",
                "adult_enabled",
                "user_level_grants",
            ),
            "crates/reading/src/repositories/library.rs": (
                "b.access_level='adult'",
                "b.access_level='pro_adult'",
                "reader_library_items",
            ),
            "crates/web/src/auth/library_actions.rs": (
                "public_book_page_for_session",
                "session_token_digest",
            ),
            "crates/recommendations/src/repositories/postgres.rs": (
                "visible_ranks",
                "u.adult_enabled",
                "user_level_grants",
            ),
            "crates/recommendations/src/repositories/feedback/discovery.rs": (
                "access_level='basic'",
                "adult_enabled",
                "user_level_grants",
            ),
            "crates/social/src/repositories/notification_read_policy.rs": (
                "b.access_level='adult'",
                "b.access_level='pro_adult'",
                "recipient.adult_enabled",
            ),
            "crates/web/tests/paid_grants_http_pg/s020_admin_age.rs": (
                "assert_admin_can_inspect_all_book_states",
                "ordinary-deleted Book",
            ),
            "crates/search/tests/actor_catalog_age_pg.rs": (
                "s020_actor_catalog_age_is_live_scoped_and_cursors_remain_exact",
                "adult_enabled=FALSE",
            ),
        }
        for path, tokens in paths_and_tokens.items():
            content = self.read(path)
            for token in tokens:
                with self.subTest(path=path, token=token):
                    self.assertIn(token, content)

    def test_source_contract_marks_this_as_coverage_not_final_signoff(self):
        report = self.read("docs/acceptance/s020-age-boundary-matrix.md")
        self.assertIn("#486", report)
        self.assertIn("не заменяет реальные испытания", report)
        self.assertIn("18+", report)
        self.assertIn("21+", report)


if __name__ == "__main__":
    unittest.main()
