"""I-07/S062 safety and real Git history regressions; no remote service needed."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import SUITES
from select_feature_tests import cargo_graph, closure, event_selection, select


def enabled(result):
    return {k for k, v in result["suites"].items() if v}


class SelectionTests(unittest.TestCase):
    def test_global_security_inputs_cannot_skip_any_available_suite(self):
        paths = [
            "Cargo.toml", "Cargo.lock", "crates/search/Cargo.toml", "rust-toolchain.toml",
            "crates/contracts/src/paid_delivery.rs", "crates/commerce/src/new_checkout.rs",
            "crates/accounts/src/services/security.rs", "crates/books/src/repositories/postgres.rs",
            "crates/reading/src/services/progress.rs", "crates/storage/src/services/crypto.rs",
            "crates/ingest/src/services/cover_jobs.rs", "migrations/202610270999_access.sql",
            "crates/web/src/auth.rs", "crates/web/assets/new-reader.js",
            "docs/contracts/paid-delivery-v1.md", "architecture/modules.toml",
            "ops/go-live-postgres-vps.sh", ".github/workflows/tor-pg-runtime.yml",
        ]
        for path in paths:
            with self.subTest(path=path):
                result = select(ROOT, [path])
                self.assertEqual(result["mode"], "full")
                self.assertEqual(enabled(result), set(SUITES))

    def test_unknown_file_new_crate_or_empty_diff_cannot_be_green_noop(self):
        for paths in ([], ["new-feature/settings.json"], ["crates/new/src/lib.rs"],
                      ["crates/search/tests/new_regression.rs"], ["../README.md"]):
            with self.subTest(paths=paths):
                self.assertEqual(enabled(select(ROOT, paths)), set(SUITES))

    def test_real_dependency_reaches_browser_for_recommendation_source(self):
        result = select(ROOT, ["crates/recommendations/src/services/ranking.rs"])
        self.assertTrue({"rust", "postgres", "reader_browser", "visual", "cover_browser",
                         "paid_e2e", "backup_recovery", "purchased_recovery"} <= enabled(result))
        self.assertIn("tor-web", result["packages"])
        self.assertNotIn("crypto", enabled(result))
        self.assertNotIn("image", enabled(result))

    def test_only_image_test_does_not_boot_postgres_or_s3(self):
        result = select(ROOT, ["crates/ingest/tests/image_pipeline.rs"])
        self.assertEqual(enabled(result), {"rust", "image"})
        self.assertEqual(result["mode"], "feature")
        self.assertTrue(result["groups"]["cheap"])

    def test_harness_edits_select_the_workflow_that_runs_them(self):
        cases = {
            "tools/visual-smoke.cjs": {"visual"},
            "tools/author_chapter_visual_smoke.cjs": {"reader_browser"},
            "tools/author_cover_pg_visual_smoke.cjs": {"cover_browser"},
            "crates/storage/tests/crypto_webcrypto.mjs": {"crypto"},
            "crates/search/tests/f02_load_pg.rs": {"rust", "search_load"},
            "tools/paid_reader_browser_smoke.cjs": {"reader_browser"},
            "tools/paid_reader_minio_chromium_e2e.cjs": {"paid_e2e"},
            "tools/paid_grant_webcrypto_e2e.mjs": {"paid_e2e", "purchased_recovery"},
            "tools/tor_dr_archive_crypto.cjs": {"backup_recovery", "purchased_recovery"},
            "tools/tor_dr_s3_backup_smoke.sh": {"backup_recovery"},
            "tools/tor_s334_paid_restore_smoke.sh": {"purchased_recovery"},
            "tools/checkout_http_pg_smoke.py": {"postgres"},
            "tools/tor_pg_restart_smoke.sh": {"postgres"},
            "tools/tor_pg_backup_restore_smoke.sh": {"postgres"},
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(enabled(select(ROOT, [path])), expected)

    def test_paid_browser_and_recovery_reports_include_their_actual_groups(self):
        paid = select(ROOT, ["tools/paid_reader_minio_chromium_e2e.cjs"])
        self.assertTrue(paid["groups"]["browser"])
        self.assertTrue(paid["groups"]["postgres"])
        backup = select(ROOT, ["tools/tor_dr_archive_crypto.cjs"])
        self.assertTrue(backup["groups"]["postgres"])
        self.assertFalse(backup["groups"]["browser"])

    def test_allowlisted_prose_runs_mandatory_rules_only(self):
        result = select(ROOT, ["README.md", "docs/ci/selection.md"])
        self.assertEqual(result["mode"], "cheap")
        self.assertEqual(enabled(result), set())
        self.assertTrue(result["groups"]["cheap"])

    def test_mix_uses_union_not_last_file(self):
        result = select(ROOT, ["README.md", "tools/visual-smoke.cjs", "crates/ingest/tests/image_pipeline.rs"])
        self.assertEqual(enabled(result), {"visual", "rust", "image"})
        result = select(ROOT, result["changed_paths"] + ["Cargo.lock"])
        self.assertEqual(enabled(result), set(SUITES))

    def test_missing_or_corrupt_graph_requires_full(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for text in (None, "invalid TOML", "[workspace]\nmembers = []"):
                if text is not None:
                    (root / "Cargo.toml").write_text(text)
                self.assertEqual(enabled(select(root, ["README.md"])), set(SUITES))

    def test_manual_main_or_unknown_event_always_full(self):
        for event in ("workflow_dispatch", "push", "merge_group", ""):
            result = event_selection(ROOT, event, {}, "a" * 40)
            self.assertEqual(enabled(result), set(SUITES))

    def test_event_missing_invalid_or_unavailable_sha_always_full(self):
        for event, head in (({}, "a" * 40), ({"pull_request": {"base": {"sha": "0" * 40}}}, "b" * 40),
                            ({"pull_request": {"base": {"sha": "a" * 40}}}, "--help"),
                            ({"pull_request": {"base": {"sha": "a" * 40}}}, "b" * 40)):
            result = event_selection(ROOT, "pull_request", event, head)
            self.assertEqual(enabled(result), set(SUITES))

    def test_machine_outputs_have_only_fixed_keys_and_boolean_values(self):
        with tempfile.TemporaryDirectory() as temp:
            output, summary = Path(temp)/"output", Path(temp)/"summary"
            subprocess.run([sys.executable, str(ROOT/"tools/select_feature_tests.py"),
                            "--path", "README.md\nsuites={}"], check=True, capture_output=True,
                           env={**os.environ, "GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(summary)})
            lines = output.read_text().splitlines()
            self.assertEqual(len(lines), 2)
            self.assertEqual(json.loads(lines[0].removeprefix("suites=")), dict.fromkeys(SUITES, True))
            self.assertEqual(lines[1], "mode=full")


class GitHistoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "CI selection test")
        self.git("config", "user.email", "ci@example.invalid")
        (self.root/"README.md").write_text("fixture\n")
        self.commit()

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True, text=True).stdout.strip()

    def commit(self):
        self.git("add", "--all")
        self.git("commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def test_rename_includes_removed_security_path_even_if_new_name_is_prose(self):
        from select_feature_tests import changed_paths
        (self.root/"auth.rs").write_text("fn auth() {}\n")
        base = self.commit()
        self.git("mv", "auth.rs", "AGENTS.md")
        head = self.commit()
        paths = changed_paths(self.root, base, head)
        self.assertEqual(set(paths), {"auth.rs", "AGENTS.md"})
        self.assertEqual(enabled(select(ROOT, paths)), set(SUITES))

    def test_complete_diff_not_github_300_file_filter(self):
        from select_feature_tests import changed_paths
        base = self.git("rev-parse", "HEAD")
        for i in range(350):
            (self.root/f"doc-{i}.md").write_text("fixture\n")
        (self.root/"z-security.rs").write_text("fn check() {}\n")
        paths = changed_paths(self.root, base, self.commit())
        self.assertEqual(len(paths), 351)
        self.assertIn("z-security.rs", paths)

    def test_merge_base_does_not_drop_branch_edits(self):
        from select_feature_tests import changed_paths
        self.git("switch", "-qc", "feature")
        (self.root/"branch.rs").write_text("fn branch() {}\n")
        head = self.commit()
        self.git("switch", "-q", "main")
        (self.root/"base.rs").write_text("fn base() {}\n")
        base = self.commit()
        self.assertEqual(changed_paths(self.root, base, head), ["branch.rs"])


class CargoGraphTests(unittest.TestCase):
    def test_alias_optional_target_and_inherited_dependencies_are_followed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/"Cargo.toml").write_text('''[workspace]
members = ["crates/web", "crates/search", "crates/contracts"]
[workspace.dependencies]
local = { package = "tor-contracts", path = "crates/contracts" }
''')
            for name in ("web", "search", "contracts"):
                directory = root/f"crates/{name}"
                directory.mkdir(parents=True)
                (directory/"Cargo.toml").write_text(f'[package]\nname = "tor-{name}"\n')
            with (root/"crates/web/Cargo.toml").open("a") as f:
                f.write('''[target.'cfg(windows)'.dependencies]
renamed = { package = "tor-search", path = "../search", optional = true }
''')
            with (root/"crates/search/Cargo.toml").open("a") as f:
                f.write('[build-dependencies]\nlocal.workspace = true\n')
            graph, _ = cargo_graph(root)
            self.assertEqual(closure(graph, ["tor-web"]), {"tor-web", "tor-search", "tor-contracts"})


if __name__ == "__main__":
    unittest.main()
