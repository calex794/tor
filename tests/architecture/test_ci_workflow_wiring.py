"""Prevent dropped CI suites, duplicate selectors or divergent manual wrappers."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import (
    ACCEPTANCE_WORKFLOWS, CI_REQUIRED_JOB, CI_WORKFLOW, ISOLATED_GATEWAY_WORKFLOWS, MANUAL_WORKFLOWS, PERFORMANCE_WORKFLOWS, POSTGRES_RUNTIME_ONLY_TEST_FILES, SUITES,
    SUITE_JOB_IDS, TEST_SUITES, workflow_gate,
)
WORKFLOWS = ROOT / ".github" / "workflows"


def job_sections(text):
    parts = text.split("\njobs:\n", 1)
    if len(parts) != 2:
        return {}
    body = parts[1]
    entries = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", body, re.M))
    return {
        item.group(1): body[item.start():(entries[i + 1].start()
                                     if i + 1 < len(entries) else len(body))]
        for i, item in enumerate(entries)
    }


def problems(text, suite):
    """Standalone suites must be manual-only and preserve their guarded job."""
    result = []
    events = text.split("\npermissions:", 1)[0] + "\n"
    if "  workflow_dispatch:\n" not in events:
        result.append("missing manual dispatch")
    for forbidden in ("  pull_request:\n", "  push:\n", "  workflow_run:\n"):
        if forbidden in events:
            result.append("manual wrapper starts automatic CI")
    if re.search(r"^\s+(paths|paths-ignore):", events, re.M):
        result.append("event-level filter is forbidden")
    sections = job_sections(text)
    if set(sections) != {"select", SUITE_JOB_IDS[suite]}:
        result.append("manual suite has missing or extra jobs")
    if "    uses: ./.github/workflows/ci-selection.yml\n" not in sections.get("select", ""):
        result.append("missing reusable planner")
    if f"      caller: {suite}\n" not in sections.get("select", ""):
        result.append("wrong suite artifact name")
    if "    needs: select\n    if: " + workflow_gate(suite) + "\n" not in sections.get(SUITE_JOB_IDS[suite], ""):
        result.append("missing full-run fallback or suite gate")
    return result


class WorkflowWiringTests(unittest.TestCase):
    def test_single_automatic_selector_and_complete_suite_jobs(self):
        text = (WORKFLOWS / CI_WORKFLOW).read_text()
        events = text.split("\npermissions:", 1)[0]
        for event in ("pull_request:", "push:", "workflow_dispatch:"):
            self.assertIn(f"  {event}", events)
        self.assertNotRegex(events, r"(?m)^\s+(paths|paths-ignore):")
        sections = job_sections(text)
        self.assertEqual(set(sections), {"select", "preflight", CI_REQUIRED_JOB} | set(SUITE_JOB_IDS.values()))
        self.assertEqual(text.count("    uses: ./.github/workflows/ci-selection.yml\n"), 1)
        self.assertIn("      caller: all\n", sections["select"])
        self.assertNotIn("pull_request_target:", text)
        for suite, (filename, _) in SUITES.items():
            with self.subTest(suite=suite):
                old = (WORKFLOWS / filename).read_text()
                self.assertEqual(problems(old, suite), [])
                job_id = SUITE_JOB_IDS[suite]
                gate = workflow_gate(suite).replace(
                    "!cancelled() && ",
                    "!cancelled() && needs.preflight.result == 'success' && ",
                    1,
                )
                self.assertIn("    needs: [select, preflight]\n    if: " + gate + "\n",
                              sections[job_id])
                # The specialized job body must remain byte-identical to its
                # manual counterpart, apart from its preflight dependency.
                manual = job_sections(old)[job_id]
                expected = manual.replace(
                    "    needs: select\n    if: " + workflow_gate(suite) + "\n",
                    "    needs: [select, preflight]\n    if: " + gate + "\n",
                    1,
                )
                self.assertNotEqual(manual, expected)
                self.assertEqual(sections[job_id], expected)

    def test_exact_postgres_test_file_selection_matches_real_job_invocations(self):
        # A new job invoking a mapped test must update the authority map and
        # trigger full CI rather than silently skipping the new consumer.
        combined = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        standalone = job_sections((WORKFLOWS / "tor-pg-runtime.yml").read_text())
        pg = combined["postgres-runtime"]
        manual_pg = standalone["postgres-runtime"]
        other_jobs = "\n".join(body for job, body in combined.items()
                               if job != "postgres-runtime")
        self.assertEqual(len(POSTGRES_RUNTIME_ONLY_TEST_FILES), 95)
        for path in sorted(POSTGRES_RUNTIME_ONLY_TEST_FILES):
            with self.subTest(path=path):
                self.assertTrue((ROOT / path).is_file())
                parts = Path(path).parts
                self.assertEqual((parts[0], parts[2]), ("crates", "tests"))
                package = "tor-" + parts[1]
                test_target = Path(path).stem
                invocation = (r"\bcargo test -p " + re.escape(package)
                              + r"(?: [^\n]*?)? --test " + re.escape(test_target)
                              + r"(?=\s|$)")
                self.assertRegex(pg, invocation)
                self.assertRegex(manual_pg, invocation)
                self.assertNotRegex(other_jobs, invocation)

    def test_postgres_chromium_installs_only_headless_shell(self):
        # Keep the real Chromium/OS dependencies; download only the browser
        # executable used by default headless Playwright journeys.
        for filename in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            with self.subTest(workflow=filename):
                pg = job_sections((WORKFLOWS / filename).read_text())["postgres-runtime"]
                self.assertIn("npx playwright install --only-shell chromium", pg)
                self.assertNotIn("npx playwright install --with-deps", pg)
                self.assertIn("npm install --no-save playwright@1.56.1", pg)

    def test_pg_restart_and_backup_reuse_d04_built_server(self):
        for filename in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            with self.subTest(workflow=filename):
                pg = job_sections((WORKFLOWS / filename).read_text())["postgres-runtime"]
                self.assertEqual(pg.count("cargo build -p tor-web\n"), 1)
                self.assertIn("      - name: D-04 real HTTP and PostgreSQL sessions", pg)
                self.assertIn("          cargo build -p tor-web\n", pg)
                for drill in (
                    "bash tools/tor_pg_restart_smoke.sh",
                    "bash tools/tor_pg_backup_restore_smoke.sh",
                ):
                    self.assertIn("          test -x ./target/debug/tor-web\n          " + drill, pg)

    def test_mail_viewports_run_parallel_without_dropping_reader_coverage(self):
        mail = (ROOT / "tools/mail_browser_pg.cjs").read_text()
        self.assertIn("Promise.all([['desktop', 1440], ['mobile', 390]].map(async", mail)
        self.assertIn("for (const client of clients) await client.context.close()", mail)
        self.assertIn("mail.${name}.${suffix}@example.test", mail)
        for filename in (CI_WORKFLOW, "tor-reader-visual.yml"):
            reader = job_sections((WORKFLOWS / filename).read_text())["reader-browser"]
            self.assertIn("node tools/reader_visual_smoke.cjs", reader)

    def test_preflight_is_fast_and_all_expensive_jobs_depend_on_it(self):
        sections = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        preflight = sections["preflight"]
        self.assertIn("    runs-on: ubuntu-24.04", preflight)
        self.assertIn("run: python3 tools/check_feature_layout.py", preflight)
        self.assertIn("run: cargo fmt --all -- --check", preflight)
        self.assertNotIn("cargo build", preflight)
        self.assertNotIn("cargo test --workspace", preflight)
        self.assertNotIn("postgres:18", preflight)
        for name in SUITE_JOB_IDS.values():
            with self.subTest(job=name):
                job = sections[name]
                self.assertIn("    needs: [select, preflight]", job)
                self.assertIn("needs.preflight.result == 'success'", job)

    def test_stable_required_ci_waits_for_all_suites(self):
        text = (WORKFLOWS / CI_WORKFLOW).read_text()
        section = job_sections(text)[CI_REQUIRED_JOB]
        deps = re.search(r"    needs:\n((?:      - [a-z0-9_-]+\n)+)", section)
        self.assertIsNotNone(deps)
        names = re.findall(r"^      - ([a-z0-9_-]+)$", deps.group(1), re.M)
        expected = {"select", "preflight"} | set(SUITE_JOB_IDS.values())
        self.assertEqual(set(names), expected)
        self.assertEqual(len(names), len(expected))
        self.assertIn("    if: ${{ always() }}\n", section)
        self.assertIn("TOR_CI_NEEDS_JSON: ${{ toJSON(needs) }}", section)
        self.assertIn("run: python3 tools/verify_ci_gate.py", section)
        self.assertIn("persist-credentials: false", section)

    def test_pr_only_concurrency_preserves_main_and_manual_runs(self):
        expected = (
            "concurrency:\n"
            "  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.run_id }}\n"
            "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n"
        )
        for filename in (CI_WORKFLOW, "rules.yml"):
            with self.subTest(workflow=filename):
                text = (WORKFLOWS / filename).read_text()
                self.assertIn(expected, text)
                self.assertEqual(text.count("\nconcurrency:\n"), 1)

    def test_removed_failure_fallback_is_rejected(self):
        old = (WORKFLOWS / "storage-crypto.yml").read_text()
        changed = old.replace("needs.select.result != 'success' || ", "")
        self.assertTrue(problems(changed, "crypto"))
        combined = (WORKFLOWS / CI_WORKFLOW).read_text()
        changed = combined.replace("needs.select.result != 'success' || ", "", 1)
        self.assertNotEqual(job_sections(combined), job_sections(changed))

    def test_manual_wrapper_cannot_restart_automatic_fanout(self):
        text = (WORKFLOWS / "tor-reader-visual.yml").read_text()
        changed = text.replace("  workflow_dispatch:\n",
                               "  pull_request:\n    paths: ['crates/reading/**']\n")
        self.assertTrue(problems(changed, "reader_browser"))

    def test_rules_are_unconditional_and_deployment_is_separate(self):
        rules = (WORKFLOWS / "rules.yml").read_text()
        self.assertNotIn("needs: select", rules)
        self.assertIn("python3 -m unittest discover -s tests/architecture -v", rules)
        for filename in MANUAL_WORKFLOWS:
            with self.subTest(workflow=filename):
                deploy = (WORKFLOWS / filename).read_text()
                self.assertNotIn("ci-selection", deploy)
                self.assertNotIn("  push:\n", deploy)
                self.assertNotIn("  pull_request:\n", deploy)
                self.assertNotIn("  workflow_run:\n", deploy)
                self.assertIn("  workflow_dispatch:\n", deploy)

    def test_isolated_performance_workflow_does_not_contact_live_hosts(self):
        for filename in PERFORMANCE_WORKFLOWS:
            with self.subTest(workflow=filename):
                contents = (WORKFLOWS / filename).read_text()
                self.assertIn("  workflow_dispatch:\n", contents)
                self.assertIn("  pull_request:\n", contents)
                self.assertIn("    paths:", contents)
                self.assertNotIn("  push:\n", contents)
                self.assertNotIn("  pull_request_target:\n", contents)
                self.assertNotIn("secrets.", contents)
                self.assertIn("127.0.0.1:3089", contents)
                self.assertIn("postgres:18", contents)
                self.assertNotIn("tor-one-click-release", contents)
        harness = (ROOT / "tests/load/tor_http.js").read_text()
        self.assertIn("http://127.0.0.1:3089", harness)
        self.assertNotIn("__ENV.BASE_URL", harness)

    def test_reusable_planner_uses_complete_history_and_no_secrets(self):
        text = (WORKFLOWS / "ci-selection.yml").read_text()
        self.assertIn("fetch-depth: 0", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("contents: read", text)
        self.assertNotIn("secrets.", text)
        self.assertNotIn("pull_request_target", text)
        self.assertIn("python3 tools/select_feature_tests.py", text)

    def test_rec13_acceptance_tests_committed_source_without_rewriting(self):
        self.assertEqual(ACCEPTANCE_WORKFLOWS, {"rec-feedback-acceptance.yml"})
        text = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        self.assertIn("contents: read", text)
        self.assertNotIn("contents: write", text)
        self.assertNotIn("pull_request_target", text)
        self.assertIn("ref: ${{ github.event.pull_request.head.sha || github.sha }}", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("git diff --exit-code", text)
        for command in (
            "cargo fmt --all -- --check",
            "cargo test -p tor-recommendations --test feedback_pg -- --ignored",
            "cargo test -p tor-recommendations --test generations_pg -- --ignored",
            "cargo test -p tor-web --test recommendation_feedback_http_pg -- --ignored",
            "bash tools/rec13_browser_acceptance.sh",
        ):
            self.assertIn(command, text)
        for forbidden in ("rec13_integrate.py", "rec13_harden.py", "rec13_publish_tree.py"):
            self.assertNotIn(forbidden, text)
        self.assertNotIn("cargo test --workspace --all-targets", text)
        self.assertNotIn("cargo clippy --workspace --all-targets", text)
        # A reader-only PR must not launch a separate REC-13 acceptance run.
        self.assertNotIn("      - 'crates/**'", text)
        for owned in (
            "crates/recommendations/**",
            "crates/web/src/recommendation_http/**",
            "crates/web/src/recommendation_worker.rs",
            "crates/web/tests/recommendation_feedback_http_pg.rs",
            "tools/rec13_*",
        ):
            self.assertIn(owned, text)
        self.assertIn("bash tools/rec13_browser_acceptance.sh", text)

    def test_rec13_migration_filter_is_exact_and_keeps_core_pg_coverage(self):
        # Each REC schema migration uses the explicit _rec_ family; a
        # non-REC migration is already covered by TOR integration/PG.
        text = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        self.assertIn("      - 'migrations/*_rec_*.sql'", text)
        self.assertNotIn("      - 'migrations/**'", text)
        migrations = [
            p.name for p in (ROOT / "migrations").glob("*.sql")
        ]
        self.assertTrue(any("_rec_" in p for p in migrations))
        self.assertTrue(any("_rec_" not in p for p in migrations))
        import fnmatch
        for name in migrations:
            with self.subTest(migration=name):
                matched = fnmatch.fnmatchcase(name, "*_rec_*.sql")
                self.assertEqual(matched, "_rec_" in name)
        core = (WORKFLOWS / CI_WORKFLOW).read_text()
        self.assertIn("  postgres-runtime:", core)
        self.assertIn("  migrations:", core)
        self.assertIn("    needs: [select, preflight]", core)

    def test_no_unknown_workflows_or_missing_harness(self):
        known = ({filename for filename, _ in SUITES.values()}
                 | {CI_WORKFLOW, "rules.yml", "ci-selection.yml"}
                 | MANUAL_WORKFLOWS | PERFORMANCE_WORKFLOWS | ISOLATED_GATEWAY_WORKFLOWS | ACCEPTANCE_WORKFLOWS)
        self.assertEqual({p.name for p in WORKFLOWS.glob("*.yml")}, known)
        self.assertEqual(set(SUITES), set(SUITE_JOB_IDS))
        self.assertEqual(len(set(SUITE_JOB_IDS.values())), len(SUITES))
        for path, suites in TEST_SUITES.items():
            with self.subTest(path=path):
                self.assertTrue((ROOT / path).is_file())
                self.assertTrue(suites <= SUITES.keys())


if __name__ == "__main__":
    unittest.main()
