"""Prevent dropped CI suites, duplicate selectors or divergent manual wrappers."""
from pathlib import Path
from ci_workflow_sources import expand_ci_actions
import re
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import (
    ACCEPTANCE_WORKFLOWS, BUILD_ONLY_WORKFLOWS, CI_REQUIRED_JOB, CI_WORKFLOW, ISOLATED_GATEWAY_WORKFLOWS, MANUAL_WORKFLOWS, PERFORMANCE_WORKFLOWS, POSTGRES_RUNTIME_ONLY_TEST_FILES, READING_ANALYTICS_TEST_FILES, SUITES,
    SUITE_JOB_IDS, TEST_SUITES, workflow_gate,
)
WORKFLOWS = ROOT / ".github" / "workflows"


def job_sections(text):
    text = expand_ci_actions(text)
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

    def test_rec13_matrix_preserves_current_history_and_scale(self):
        script_path = ROOT / "tools/rec13_browser_acceptance.sh"
        script = script_path.read_text()
        # Fast fail for a broken Bash conditional before expensive Chromium/PG.
        subprocess.run(["bash", "-n", str(script_path)], check=True)
        self.assertIn('rec_lane="${TOR_REC13_LANE:-all}"', script)
        self.assertIn("all|current|historical) ;;", script)
        self.assertIn('if [[ "$rec_lane" != historical ]]; then', script)
        self.assertIn('if [[ "$rec_lane" != current ]]; then', script)
        self.assertLess(
            script.index('if [[ "$rec_lane" != historical ]]; then'),
            script.index('if [[ "$rec_lane" != current ]]; then'),
        )
        # #1134: keep EDITORIAL acceptance independent of an earlier degraded
        # collector. The replacement must have a real new SQL epoch and live
        # lease; the browser must still insist on actual 204/durable counters.
        fresh = script[script.index('old_editorial_epoch='):
                       script.index('node tools/editorial_pg_browser.cjs')]
        for required in (
            'stop_server',
            'start_server "$TEST_DATABASE_URL" current',
            'new_editorial_epoch=',
            'new_editorial_epoch <= old_editorial_epoch',
            'lease_until_ms>',
            "CI_EDITORIAL_FRESH_EPOCH_OK",
            'TOR_EDITORIAL_EXPECT_OBSERVATIONS=1',
        ):
            self.assertIn(required, fresh)
        editorial = (ROOT / "tools/editorial_pg_browser.cjs").read_text()
        self.assertIn("event.status===204", editorial)
        self.assertIn("sums(campaign)[2]>before", editorial)
        self.assertIn("This card has no accepted view callback", editorial)
        # The specialist workflow invokes the unfiltered all mode.
        specialist = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        self.assertIn("run: bash tools/rec13_browser_acceptance.sh", specialist)
        self.assertNotIn("TOR_REC13_LANE:", specialist)
        for workflow in (CI_WORKFLOW, "tor-pg-rec-browser.yml"):
            with self.subTest(workflow=workflow):
                body = job_sections((WORKFLOWS / workflow).read_text())["pg-rec-browser"]
                self.assertIn("lane: [current, historical]", body)
                self.assertIn("fail-fast: false", body)
                self.assertIn("image: postgres:18", body)
                self.assertIn("TOR_REC13_LANE: ${{ matrix.lane }}", body)
                self.assertIn("run: bash tools/rec13_browser_acceptance.sh", body)
                self.assertIn(
                    "name: pg-rec-browser-${{ github.sha }}-${{ matrix.lane }}", body
                )
                historical = "if: ${{ matrix.lane == 'historical' }}"
                current = "if: ${{ matrix.lane == 'current' }}"
                self.assertEqual(body.count(historical), 2)
                self.assertEqual(body.count(current), 1)
                self.assertLess(
                    body.index(historical),
                    body.index("collection_retirement_installed_upgrade_pg"),
                )
                self.assertIn(
                    "contextual_scale::model_page_is_bounded_with_100k_books_and_reader_requests",
                    body,
                )

    def test_100k_contextual_scale_owns_only_one_isolated_pg_lane(self):
        # Every test still runs, but the heavy scale case is removed only
        # from the sequential runtime lane, not from required CI coverage.
        test_name = "contextual_scale::model_page_is_bounded_with_100k_books_and_reader_requests"
        pkg = "cargo test -p tor-recommendations --test catalog_feed_pg"
        runtime = pkg + " -- --ignored --test-threads=1 --skip " + test_name
        isolated = pkg + " " + test_name + " -- --ignored --exact --test-threads=1"
        for name in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            with self.subTest(runtime=name):
                pg = job_sections((WORKFLOWS / name).read_text())["postgres-runtime"]
                self.assertIn(runtime, pg)
                self.assertEqual(pg.count(pkg), 1)
        for name in (CI_WORKFLOW, "tor-pg-rec-browser.yml"):
            with self.subTest(isolated=name):
                pg = job_sections((WORKFLOWS / name).read_text())["pg-rec-browser"]
                self.assertIn(isolated, pg)
                self.assertEqual(pg.count(pkg), 1)
        self.assertEqual(
            TEST_SUITES["crates/recommendations/tests/catalog_feed_pg.rs"],
            {"rust", "postgres", "rec_browser"},
        )
        self.assertEqual(
            TEST_SUITES["crates/recommendations/tests/catalog_feed_pg/contextual_scale.rs"],
            {"rust", "rec_browser"},
        )


    def test_package_cover_pg_regression_runs_on_both_postgres_workflows(self):
        command = (
            "cargo test -p tor-commerce --test layers_pg h_ui_18_ "
            "-- --ignored --test-threads=1"
        )
        for filename in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            with self.subTest(workflow=filename):
                job = job_sections((WORKFLOWS / filename).read_text())["postgres-runtime"]
                self.assertEqual(job.count(command), 1)

    def test_reading_analytics_isolated_exact_36_test_targets(self):
        self.assertEqual(len(READING_ANALYTICS_TEST_FILES), 36)
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        manual = job_sections((WORKFLOWS / "tor-pg-reading-analytics.yml").read_text())
        for body in (required["pg-reading-analytics"], manual["pg-reading-analytics"]):
            self.assertIn("image: postgres:18", body)
            self.assertIn("LEGACY_ANALYTICS_DATABASE_URL:", body)
            self.assertIn("LEGACY_ANALYTICS_MIGRATOR_URL:", body)
            self.assertIn("COLLECTION_RETIREMENT_UPGRADE_DATABASE_URL:", body)
            self.assertIn("COLLECTION_RETIREMENT_STARTUP_MIGRATOR_URL:", body)
            self.assertIn("legacy_analytics_baseline_pg", body)
            self.assertIn("collection_retirement_startup_baseline_pg", body)
            self.assertIn("collection_retirement_installed_upgrade_pg", body)
            self.assertIn("run: git diff --exit-code", body)
            for path in READING_ANALYTICS_TEST_FILES:
                package = "tor-" + Path(path).parts[1]
                target = Path(path).stem
                invocation = (f"cargo test -p {package} --test {target} "
                              "-- --ignored --test-threads=1")
                with self.subTest(path=path):
                    self.assertEqual(body.count(invocation), 1)
                    self.assertEqual(TEST_SUITES[path], {"rust", "reading_analytics"})
        self.assertEqual(
            TEST_SUITES["crates/search/tests/seo_pg/analytics.rs"],
            {"rust", "reading_analytics"},
        )
        seo_scale = (ROOT / "crates/search/tests/seo_pg/analytics.rs").read_text()
        self.assertIn("flow_seo_pg_fifty_thousand_pages_keyset_and_select_only_report", seo_scale)
        self.assertIn("generate_series(0,49999)", seo_scale)
        for name in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            runtime = job_sections((WORKFLOWS / name).read_text())["postgres-runtime"]
            self.assertIn("cargo test -p tor-reading --test library_postgres", runtime)
            for path in READING_ANALYTICS_TEST_FILES:
                package = "tor-" + Path(path).parts[1]
                target = Path(path).stem
                self.assertNotIn(f"cargo test -p {package} --test {target} ", runtime)


    def test_exact_postgres_test_file_selection_matches_real_job_invocations(self):
        # A new job invoking a mapped test must update the authority map and
        # trigger full CI rather than silently skipping the new consumer.
        combined = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        standalone = job_sections((WORKFLOWS / "tor-pg-runtime.yml").read_text())
        pg = combined["postgres-runtime"]
        manual_pg = standalone["postgres-runtime"]
        other_jobs = "\n".join(body for job, body in combined.items()
                               if job != "postgres-runtime")
        self.assertEqual(len(POSTGRES_RUNTIME_ONLY_TEST_FILES), 101)
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
                self.assertIn("          bash tools/ci_prepare_chromium.sh install\n", pg)
                self.assertNotIn("npx playwright install --with-deps", pg)

    def test_pg_restart_and_backup_reuse_d04_built_server(self):
        for filename in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            with self.subTest(workflow=filename):
                pg = job_sections((WORKFLOWS / filename).read_text())["postgres-runtime"]
                self.assertEqual(pg.count("cargo build -p tor-web\n"), 1)
                self.assertIn("      - name: D-04 real HTTP and PostgreSQL sessions", pg)
                self.assertIn("          cargo build -p tor-web\n", pg)
                self.assertIn("          test -x ./target/debug/tor-web\n"
                              "          bash tools/tor_pg_restart_smoke.sh", pg)
                # Keep the real web build reused; v2 signs the real migrator
                # and trace writer too, without starting either executable.
                self.assertIn("          test -x ./target/debug/tor-web\n"
                              "          test -x ./target/debug/tor-migrate\n"
                              "          cargo build -p tor-observability --bin tor-trace-store\n"
                              "          bash tools/tor_pg_backup_restore_smoke.sh", pg)

    def test_reader_and_visual_use_headless_chromium_shell(self):
        # Manual and required jobs retain their independent install contracts.
        integration = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        for job, manual_file in (
            ("reader-browser", "tor-reader-visual.yml"),
            ("visual", "visual-smoke.yml"),
        ):
            with self.subTest(job=job):
                for body in (
                    integration[job],
                    job_sections((WORKFLOWS / manual_file).read_text())[job],
                ):
                    if job == "visual":
                        self.assertEqual(body.count("          bash tools/ci_prepare_chromium.sh install-and-probe\n"), 1)
                        self.assertNotIn("npx playwright install --with-deps", body)
                    else:
                        self.assertEqual(body.count("          bash tools/ci_prepare_chromium.sh install\n"), 1)
                        self.assertIn("npx playwright install --with-deps --only-shell chromium", body)

    def test_cover_and_paid_e2e_use_only_headless_chromium_shell(self):
        integration = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        for job, manual_filename in (
            ("author-cover", "tor-cover-browser.yml"),
            ("paid-private-e2e", "tor-paid-grant-e2e.yml"),
        ):
            with self.subTest(job=job):
                for body in (
                    integration[job],
                    job_sections((WORKFLOWS / manual_filename).read_text())[job],
                ):
                    self.assertEqual(body.count("          bash tools/ci_prepare_chromium.sh install-and-probe\n"), 1)
                    self.assertNotIn("npx playwright install --with-deps", body)
        paid = integration["paid-private-e2e"]
        self.assertIn("      - name: Install real Chromium before issuing short-lived S3 URLs\n", paid)

    def test_shared_chromium_setup_offline_modes_and_failures(self):
        import tempfile

        script = ROOT / "tools/ci_prepare_chromium.sh"
        code = script.read_text()
        self.assertEqual(code.count("npm install --no-save playwright@1.56.1"), 1)
        self.assertEqual(code.count("npx playwright install --only-shell chromium"), 1)
        self.assertEqual(code.count("chromium.launch({headless:true})"), 1)
        self.assertNotIn("--with-deps", code)
        subprocess.run(["bash", "-n", str(script)], check=True)
        for args, fail, expected, exit_code in (
            (["install"], "", ["npm", "npx"], 0),
            (["install-and-probe"], "", ["npm", "npx", "node"], 0),
            ([], "", [], 2),
            (["invalid"], "", [], 2),
            (["install-and-probe"], "npm", ["npm"], 9),
            (["install-and-probe"], "npx", ["npm", "npx"], 9),
            (["install-and-probe"], "node", ["npm", "npx", "node"], 9),
        ):
            with self.subTest(args=args, fail=fail):
                with tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    trace = root / "trace"
                    for name in ("npm", "npx", "node"):
                        exe = root / name
                        exe.write_text(
                            '#!/bin/bash\n'
                            'printf "%s %s\\n" "${0##*/}" "$*" >> "$TRACE"\n'
                            'if [[ "${0##*/}" == "$FAIL" ]]; then exit 9; fi\n'
                        )
                        exe.chmod(0o700)
                    result = subprocess.run(
                        ["/bin/bash", str(script), *args],
                        env={"PATH": tmp, "TRACE": str(trace), "FAIL": fail},
                        capture_output=True, text=True, check=False,
                    )
                    commands = trace.read_text().splitlines() if trace.exists() else []
                    self.assertEqual([c.split()[0] for c in commands], expected)
                    self.assertEqual(result.returncode, exit_code)
                    if "npm" in expected:
                        self.assertEqual(commands[0], "npm install --no-save playwright@1.56.1")
                    if "npx" in expected:
                        self.assertEqual(commands[1], "npx playwright install --only-shell chromium")
                    if "node" in expected:
                        self.assertIn("chromium.launch({headless:true})", commands[2])

    def test_browser_mirror_preparation_precedes_any_package_install(self):
        integration = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        for job, manual_file in (
            ("reader-browser", "tor-reader-visual.yml"),
            ("visual", "visual-smoke.yml"),
            ("author-cover", "tor-cover-browser.yml"),
        ):
            with self.subTest(job=job):
                for body in (
                    integration[job],
                    job_sections((WORKFLOWS / manual_file).read_text())[job],
                ):
                    prepare = body.index("bash tools/ci_prepare_ubuntu_mirror.sh")
                    self.assertLess(body.index("uses: actions/checkout@v4"), prepare)
                    self.assertLess(prepare, body.index("bash tools/ci_prepare_chromium.sh "))
                    if "bash tools/ci_prepare_pg_client.sh" in body:
                        self.assertLess(prepare, body.index("bash tools/ci_prepare_pg_client.sh"))
                    if "sudo apt-get update" in body:
                        self.assertLess(prepare, body.index("sudo apt-get update"))

    def test_mail_viewports_run_parallel_without_dropping_reader_coverage(self):
        mail = (ROOT / "tools/mail_browser_pg.cjs").read_text()
        self.assertIn("Promise.all([['desktop', 1440], ['mobile', 390]].map(async", mail)
        self.assertIn("for (const client of clients) await client.context.close()", mail)
        self.assertIn("mail.${name}.${suffix}@example.test", mail)
        for filename in (CI_WORKFLOW, "tor-reader-visual.yml"):
            reader = job_sections((WORKFLOWS / filename).read_text())["reader-browser"]
            self.assertIn("ci_timed_browser tools/reader_visual_smoke.cjs", reader)
            self.assertIn('if node "$scenario" "$@"; then', reader)

    def test_rec13_runs_in_its_own_isolated_pg18_job(self):
        script = (ROOT / "tools/rec13_browser_acceptance.sh").read_text()
        self.assertIn("TOR_REC_REUSE_PLAYWRIGHT:-0", script)
        self.assertIn("npm install --prefix", script)
        self.assertIn("install --only-shell chromium", script)
        self.assertNotIn("install --with-deps chromium", script)
        self.assertIn("require('playwright/package.json').version !== '1.56.1'", script)
        self.assertIn("p.chromium.launch({headless:true}).then(b=>b.close())", script)
        self.assertNotIn("p.chromium.executablePath()", script)
        self.assertIn('NODE_PATH="$node_path"', script)
        for filename in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            core = job_sections((WORKFLOWS / filename).read_text())["postgres-runtime"]
            self.assertIn("          bash tools/ci_prepare_chromium.sh install\n", core)
            self.assertNotIn("bash tools/rec13_browser_acceptance.sh", core)
        for filename in (CI_WORKFLOW, "tor-pg-rec-browser.yml"):
            browser = job_sections((WORKFLOWS / filename).read_text())["pg-rec-browser"]
            self.assertIn("image: postgres:18", browser)
            self.assertIn("TEST_DATABASE_URL: postgresql://tor_app:", browser)
            self.assertIn("LEGACY_ANALYTICS_DATABASE_URL:", browser)
            self.assertIn("COLLECTION_RETIREMENT_APP_DATABASE_URL:", browser)
            self.assertIn("legacy_analytics_baseline_pg", browser)
            self.assertIn("collection_retirement_startup_baseline_pg", browser)
            self.assertIn("collection_retirement_installed_upgrade_pg", browser)
            self.assertIn("run: bash tools/rec13_browser_acceptance.sh", browser)
            self.assertNotIn("TOR_REC_REUSE_PLAYWRIGHT=1", browser)
            self.assertIn("actions/upload-artifact@v4", browser)
        specialist = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        self.assertIn("run: bash tools/rec13_browser_acceptance.sh", specialist)
        self.assertIn("TOR_REC_REUSE_PLAYWRIGHT: '1'", specialist)

    def test_rec_specialist_chromium_reuse_keeps_launch_and_fallback(self):
        specialist = (WORKFLOWS / "rec-feedback-acceptance.yml").read_text()
        install = specialist.split(
            "      - name: Chromium for native fresh-schema HTML quality\n", 1
        )[1].split(
            "      - name: Native HTML quality on the fresh schema\n", 1
        )[0]
        self.assertIn("          bash tools/ci_prepare_chromium.sh install\n", install)
        self.assertIn("npx playwright install --with-deps --only-shell chromium", install)
        self.assertIn("p.chromium.launch({headless:true})", install)
        self.assertIn("if ! rec_chromium_probe; then", install)
        self.assertIn("CI_REC_CHROMIUM_SETUP_SECONDS", install)
        self.assertLess(
            install.index("bash tools/ci_prepare_chromium.sh install"),
            install.index("npx playwright install --with-deps --only-shell chromium"),
        )
        self.assertLess(
            install.index("npx playwright install --with-deps --only-shell chromium"),
            install.rindex("rec_chromium_probe"),
        )
        browser_step = specialist.split("      - name: Browser acceptance\n", 1)[1]
        self.assertIn("TOR_REC_REUSE_PLAYWRIGHT: '1'", browser_step)
        self.assertIn("run: bash tools/rec13_browser_acceptance.sh", browser_step)

    def test_reader_chromium_launch_preflight_with_dependency_fallback(self):
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["reader-browser"]
        manual = job_sections((WORKFLOWS / "tor-reader-visual.yml").read_text())["reader-browser"]
        for name, body in (("required", required), ("manual", manual)):
            with self.subTest(workflow=name):
                step = body.split("      - name: Install Chromium\n", 1)[1].split(
                    "      - name: Browse chapter, TOC, mobile layout and locked URLs\n", 1
                )[0]
                self.assertIn("          bash tools/ci_prepare_chromium.sh install\n", step)
                self.assertIn("npx playwright install --with-deps --only-shell chromium", step)
                self.assertIn('p.chromium.launch({headless:true})', step)
                self.assertIn('version !== "1.56.1"', step)
                self.assertIn("if ! chromium_probe; then", step)
                self.assertEqual(step.count("          chromium_probe\n"), 1)
                self.assertLess(
                    step.index("bash tools/ci_prepare_chromium.sh install"),
                    step.index("npx playwright install --with-deps --only-shell chromium"),
                )
                self.assertIn("CI_BROWSER_CHROMIUM_SETUP_SECONDS", step)
                # A successful install alone cannot satisfy the check: browser must launch.
                self.assertLess(
                    step.index("npx playwright install --with-deps --only-shell chromium"),
                    step.rindex("chromium_probe"),
                )

    def test_reader_scenario_timing_preserves_manual_and_required_coverage(self):
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["reader-browser"]
        manual = job_sections((WORKFLOWS / "tor-reader-visual.yml").read_text())["reader-browser"]
        expected = (
            "reader_visual_smoke.cjs",
            "br060_archive_browser_pg.cjs",
            "author_chapter_visual_smoke.cjs",
            "loop_01_browser_smoke.cjs",
            "chapter_file_import_browser.cjs",
            "home_spotlight_visual_smoke.cjs",
            "auth_dialog_browser.cjs",
            "search_visual_smoke.cjs",
            "ranking_visual_smoke.cjs",
            "paid_reader_browser_smoke.cjs",
            "batch_checkout_chromium_pg.cjs",
            "referral_share_browser_smoke.cjs",
            "editorial_pg_browser.cjs",
            "s039_payout_post_browser.cjs",
        )
        commands = []
        for name, body in (("required", required), ("manual", manual)):
            with self.subTest(workflow=name):
                self.assertIn("CI_BROWSER_SCENARIO_SECONDS", body)
                self.assertIn('return "$result"', body)
                self.assertIn('if node "$scenario" "$@"; then', body)
                self.assertIn("set -euo pipefail", body)
                found = tuple(re.findall(r"ci_timed_browser tools/([\w-]+\.cjs)", body))
                self.assertEqual(found, expected)
                commands.append(found)
        self.assertEqual(commands[0], commands[1])

    def test_reader_and_search_browser_waits_keep_real_page_assertions(self):
        # The real Chromium runs, screenshots and SQL-backed assertions remain;
        # only waiting for unrelated idle network connections is removed.
        for filename in ("reader_visual_smoke.cjs", "search_visual_smoke.cjs"):
            with self.subTest(script=filename):
                code = (ROOT / "tools" / filename).read_text()
                self.assertNotIn("'networkidle'", code)
                self.assertIn("waitUntil: 'load'", code)
                self.assertIn("chromium.launch({ headless: true })", code)
                self.assertIn("screenshot({", code)
        reader = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["reader-browser"]
        for filename in ("reader_visual_smoke.cjs", "search_visual_smoke.cjs"):
            self.assertIn("ci_timed_browser tools/" + filename, reader)

    def test_reader_visual_phase_timing_preserves_serial_postgres_journeys(self):
        script = (ROOT / "tools/reader_visual_smoke.cjs").read_text()
        self.assertIn("const { performance } = require('node:perf_hooks');", script)
        self.assertIn("CI_BROWSER_PHASE_SECONDS scenario=", script)
        self.assertIn("CI_BROWSER_IO_SECONDS scenario=", script)
        self.assertIn("reportPhase('total', 'all', suiteStarted)", script)
        self.assertIn("reportIoTotals();", script)
        self.assertEqual(script.count("for (const [name, width, height] of"), 3)
        self.assertNotIn("Promise.all([['desktop'", script)
        self.assertEqual(script.count("await page.screenshot({"), 1)
        self.assertIn("fullPage: true", script)
        for phase in (
            "guest_toc", "guest_paywall", "guest_comments", "guest_reader",
            "guest_denials", "theme_light", "registration_validation",
            "registration_success", "reader_resume", "chapter_discussion",
            "completion_ranking", "login_and_errors", "comment_spoilers",
            "mail_browser", "support_browser",
        ):
            with self.subTest(phase=phase):
                self.assertIn(phase, script)
        # Reader viewports and spoiler checks remain serial. Complete,
        # fixture-isolated mail/support journeys run concurrently, and both
        # failures must propagate after the shared Chromium has finished.
        self.assertIn("await require('./comment_spoilers_browser.cjs')(browser, base, out);", script)
        self.assertIn("['mail_browser', './mail_browser_pg.cjs']", script)
        self.assertIn("['support_browser', './support_browser_pg.cjs']", script)
        self.assertEqual(script.count("const results = await Promise.allSettled(auxiliary.map"), 1)
        self.assertIn("await require(modulePath)(browser, base, out);", script)
        self.assertIn("reportPhase(name, 'all', started);", script)
        self.assertIn("reportPhase('auxiliary_parallel_total', 'all', auxiliaryStarted);", script)
        self.assertIn("const failures = results.filter(result => result.status === 'rejected');", script)
        self.assertIn("throw new AggregateError(failures.map(result => result.reason)", script)
        self.assertLess(
            script.index("await require('./comment_spoilers_browser.cjs')"),
            script.index("const results = await Promise.allSettled(auxiliary.map"),
        )

        mail = (ROOT / "tools/mail_browser_pg.cjs").read_text()
        self.assertIn("CI_BROWSER_MAIL_PHASE_SECONDS scenario=", mail)
        self.assertIn("await Promise.all([['desktop', 1440], ['mobile', 390]]", mail)
        self.assertIn("for (let n = 0; n < 53; n++)", mail)
        self.assertEqual(mail.count("reportMailPhase(name, '"), 16)
        self.assertIn("reportMailPhase(name, 'group_transfer', checkpoint)", mail)
        self.assertIn("reportMailPhase(name, 'context_cleanup', checkpoint)", mail)

        # BrowserContext-scoped fake timers must never change real mailbox,
        # outbox or notification delivery timing in the authenticated context.
        self.assertIn("const probeContext = await browser.newContext({", mail)
        self.assertIn("storageState: await b.context.storageState()", mail)
        self.assertIn("await probe.clock.install();", mail)
        self.assertEqual(mail.count("await probe.clock.fastForward(3000);"), 2)
        self.assertIn("await probeContext.close();", mail)
        self.assertNotIn("const probe = await b.context.newPage();", mail)
        self.assertIn("const nextPoll = probe.waitForResponse", mail)
        self.assertIn("assert(seenCursors.includes(largeId)", mail)

    def test_pg_timing_keeps_six_ordered_phases_and_full_coverage(self):
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["postgres-runtime"]
        manual = job_sections((WORKFLOWS / "tor-pg-runtime.yml").read_text())["postgres-runtime"]
        phases = (
            "traffic_storage_catalog", "books_import_authoring",
            "reading_library", "referrals_recommendations_social",
            "commerce_support", "http_security",
        )
        for job in (required, manual):
            body = job.split("      - name: Database and HTTP integration\n", 1)[1].split(
                "      - name: D-04 real HTTP and PostgreSQL sessions", 1)[0]
            self.assertEqual(
                tuple(re.findall(r'ci_pg_phase_done "([a-z_]+)"', body)), phases)
            self.assertIn("CI_PG_PHASE_SECONDS phase=$phase", body)
            self.assertIn("ci_pg_started=$SECONDS", body)
            self.assertIn("cargo test -p tor-web --test paid_grants_http_pg", body)
            self.assertNotIn("bash tools/rec13_browser_acceptance.sh", body)
            self.assertIn("cargo test -p tor-web --test ril_behavior_consent_pg", body)

    def test_postgres_cargo_command_timing_keeps_fail_fast_and_targets(self):
        # Timing is shell-local and does not change any cargo test arguments,
        # filters, test binaries, shared PostgreSQL ordering or the manual job.
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["postgres-runtime"]
        manual = job_sections((WORKFLOWS / "tor-pg-runtime.yml").read_text())["postgres-runtime"]
        expected = (
            "traffic_storage_catalog", "books_import_authoring",
            "reading_library", "referrals_recommendations_social",
            "commerce_support", "http_security",
        )
        for job in (required, manual):
            body = job.split("      - name: Database and HTTP integration\n", 1)[1].split(
                "      - name: D-04 real HTTP and PostgreSQL sessions", 1)[0]
            self.assertEqual(tuple(re.findall(r'ci_pg_phase_done "([a-z_]+)"', body)), expected)
            self.assertIn("CI_PG_CARGO_MS seq=%d package=%s target=%s elapsed_ms=%d status=%d", body)
            self.assertIn('command cargo "$@" || status=$?', body)
            self.assertIn('return "$status"', body)
            self.assertIn('local start_us=${EPOCHREALTIME/./}', body)
            self.assertIn('local end_us=${EPOCHREALTIME/./}', body)
            self.assertIn('env -u TOR_DIRECT_SWAP_BROWSER_ENABLED -u TOR_PAID_CHECKOUT_ENABLED cargo test', body)
            self.assertNotIn("bash tools/rec13_browser_acceptance.sh", body)
            self.assertGreaterEqual(len(re.findall(r'(?m)^\s+cargo test -p ', body)), 100)
            self.assertNotIn("set +e", body)

    def test_rec13_step_timing_keeps_real_benchmark_and_failure_handling(self):
        shell = (ROOT / "tools/rec13_browser_acceptance.sh").read_text()
        browser = (ROOT / "tools/rec13_browser.cjs").read_text()
        for stage in ("cargo_build", "playwright_setup", "http_ready", "browser_acceptance"):
            self.assertIn('ci_rec_step_done "' + stage + '"', shell)
        self.assertIn("CI_REC13_STEP_SECONDS stage=$stage", shell)
        self.assertIn("set -Eeuo pipefail", shell)
        self.assertIn("node tools/rec13_browser.cjs", shell)
        self.assertIn("await benchmarkOrganicActivation({", browser)
        self.assertIn("CI_REC13_STEP_SECONDS stage=organic_post_load", browser)
        self.assertIn("REC13 browser PASS:", browser)

    def test_other_postgres_jobs_use_runner_client_with_apt_fallback(self):
        integration = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        command = "bash tools/ci_prepare_pg_client.sh"
        pairs = (
            ("postgres-runtime", "tor-pg-runtime.yml"),
            ("reader-browser", "tor-reader-visual.yml"),
            ("author-cover", "tor-cover-browser.yml"),
            ("s3-cover-build", "tor-cover-s3.yml"),
            ("paid-private-e2e", "tor-paid-grant-e2e.yml"),
            ("sealed-remote-copy", "tor-disaster-recovery.yml"),
            ("purchased-v1-recovery", "tor-s334-restore.yml"),
        )
        for name, filename in pairs:
            with self.subTest(job=name):
                manual = job_sections((WORKFLOWS / filename).read_text())[name]
                for body in (integration[name], manual):
                    self.assertEqual(body.count(command), 1)
                    self.assertNotIn("command -v psql", body)
        self.assertEqual(len(pairs), 7)
        isolated = (ROOT / "tools/ci_prepare_isolated_pg.sh").read_text()
        self.assertEqual(isolated.count(command), 1)
        # The migrations job intentionally retains its different apt verbosity.
        for body in (integration["migrations"],
                     job_sections((WORKFLOWS / "postgres-migrations.yml").read_text())["migrations"]):
            self.assertIn("command -v psql", body)
            self.assertIn("command -v pg_isready", body)
            self.assertIn("psql --version", body)
            self.assertIn("Using preinstalled PostgreSQL client", body)
            self.assertIn("sudo apt-get update\n", body)
            self.assertIn("sudo apt-get install -y postgresql-client", body)

    def test_pg_client_uses_existing_runner_binaries_with_safe_fallback(self):
        import tempfile

        script = ROOT / "tools/ci_prepare_pg_client.sh"
        code = script.read_text()
        for command in (
            "command -v psql", "command -v pg_isready", "psql --version",
            "sudo apt-get update -qq", "sudo apt-get install -y postgresql-client",
            "Using preinstalled PostgreSQL client",
        ):
            self.assertIn(command, code)
        subprocess.run(["bash", "-n", str(script)], check=True)
        # Exercise both branches and failure propagation with a fake PATH.
        # No apt, sudo or network commands can reach the actual runner.
        for has_psql, has_ready, fail_update in (
            (True, True, False), (True, False, False),
            (False, True, False), (False, False, False),
            (False, False, True),
        ):
            with self.subTest(psql=has_psql, pg_isready=has_ready, fail=fail_update):
                with tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    trace = root / "trace"
                    for name in ("sudo", "psql", "pg_isready"):
                        if (name == "psql" and not has_psql) or (name == "pg_isready" and not has_ready):
                            continue
                        exe = root / name
                        exe.write_text(
                            '#!/bin/bash\n'
                            'printf "%s %s\\n" "${0##*/}" "$*" >> "$TRACE"\n'
                            'if [[ "${0##*/}" == sudo && "${2:-}" == update ]]; then\n'
                            '  exit "${UPDATE_EXIT:-0}"\n'
                            'fi\n'
                        )
                        exe.chmod(0o700)
                    result = subprocess.run(
                        ["/bin/bash", str(script)],
                        env={"PATH": tmp, "TRACE": str(trace),
                             "UPDATE_EXIT": "9" if fail_update else "0"},
                        capture_output=True, text=True, check=False,
                    )
                    expected = (["psql --version"] if has_psql and has_ready else
                                ["sudo apt-get update -qq"] if fail_update else
                                ["sudo apt-get update -qq", "sudo apt-get install -y postgresql-client"])
                    self.assertEqual(trace.read_text().splitlines(), expected)
                    self.assertEqual(result.returncode, 9 if fail_update else 0)

    def test_shared_isolated_pg_bootstrap_keeps_all_five_callers(self):
        script = ROOT / "tools/ci_prepare_isolated_pg.sh"
        code = script.read_text()
        step = ("      - name: Prepare isolated database\n"
                "        run: bash tools/ci_prepare_isolated_pg.sh\n"
                "      - name: Apply real SQLx migrations twice\n")
        for filename, count in (
            (CI_WORKFLOW, 2), ("tor-pg-reading-analytics.yml", 1),
            ("tor-pg-rec-browser.yml", 1), ("rec-feedback-acceptance.yml", 1),
        ):
            with self.subTest(workflow=filename):
                text = expand_ci_actions((WORKFLOWS / filename).read_text())
                self.assertEqual(text.count(step), count)
                self.assertEqual(text.count("run: bash tools/ci_prepare_isolated_pg.sh"), count)
                self.assertNotIn("      - name: Prepare isolated database\n        run: |", text)
                self.assertGreaterEqual(text.count("PGPASSWORD: isolated-ci-only"), count)
        for sql in (
            "CREATE ROLE tor_migrator LOGIN PASSWORD 'isolated-migrate';",
            "CREATE ROLE tor_app LOGIN PASSWORD 'isolated-app';",
            "CREATE DATABASE tor_ci OWNER tor_migrator;",
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC;",
            "GRANT USAGE ON SCHEMA public TO tor_app;",
            "ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;",
            "ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public GRANT USAGE,SELECT ON SEQUENCES TO tor_app;",
        ):
            self.assertEqual(code.count(sql), 1)
        self.assertEqual(code.count("psql -X -v ON_ERROR_STOP=1"), 2)
        subprocess.run(["bash", "-n", str(script)], check=True)
        # No external executable can run in this negative control.
        result = subprocess.run(
            ["/bin/bash", str(script)],
            env={"PATH": "/nonexistent", "GITHUB_ACTIONS": "false"},
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("Refusing non-isolated PostgreSQL CI bootstrap", result.stderr)

    def test_shared_legacy_analytics_bootstrap_keeps_all_seven_callers(self):
        script = ROOT / "tools/ci_prepare_legacy_analytics.sh"
        code = script.read_text()
        step_name = "      - name: Prepare explicit legacy and installed-upgrade analytics databases\n"
        command = "          bash tools/ci_prepare_legacy_analytics.sh\n"
        baseline_tests = (
            "          cargo test -p tor-migrations --test legacy_analytics_baseline_pg -- --ignored --test-threads=1\n"
            "          cargo test -p tor-migrations --test collection_retirement_startup_baseline_pg -- --ignored --test-threads=1\n"
        )
        historical = "        if: ${{ matrix.lane == 'historical' }}\n"
        for filename, job, condition in (
            (CI_WORKFLOW, "postgres-runtime", ""),
            (CI_WORKFLOW, "pg-rec-browser", historical),
            (CI_WORKFLOW, "pg-reading-analytics", ""),
            ("tor-pg-runtime.yml", "postgres-runtime", ""),
            ("rec-feedback-acceptance.yml", "acceptance", ""),
            ("tor-pg-reading-analytics.yml", "pg-reading-analytics", ""),
            ("tor-pg-rec-browser.yml", "pg-rec-browser", historical),
        ):
            with self.subTest(workflow=filename, job=job):
                body = job_sections((WORKFLOWS / filename).read_text())[job]
                self.assertEqual(body.count(step_name + condition + "        run: |\n" + command + baseline_tests), 1)
                self.assertEqual(body.count(command), 1)
                self.assertNotIn("CREATE DATABASE tor_ci_legacy_analytics OWNER tor_migrator;", body)
                self.assertIn("collection_retirement_installed_upgrade_pg", body)
        for sql in (
            "CREATE DATABASE tor_ci_legacy_analytics OWNER tor_migrator;",
            "CREATE DATABASE tor_ci_collection_retirement_upgrade OWNER tor_migrator;",
            "CREATE DATABASE tor_ci_collection_retirement_startup OWNER tor_migrator;",
            'REVOKE ALL ON DATABASE :"fixture_database" FROM PUBLIC;',
            'GRANT CONNECT ON DATABASE :"fixture_database" TO tor_app;',
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC;",
            "GRANT USAGE ON SCHEMA public TO tor_app;",
            "GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;",
            "GRANT USAGE,SELECT ON SEQUENCES TO tor_app;",
        ):
            self.assertEqual(code.count(sql), 1)
        self.assertEqual(code.count("psql -X -v ON_ERROR_STOP=1"), 2)
        self.assertNotIn("cargo test", code)
        subprocess.run(["bash", "-n", str(script)], check=True)
        result = subprocess.run(
            ["/bin/bash", str(script)],
            env={"PATH": "/nonexistent", "GITHUB_ACTIONS": "false"},
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("Refusing non-isolated legacy analytics CI bootstrap", result.stderr)

    def test_s3_clippy_runs_once_on_selected_parallel_lane(self):
        # Keep required and manual work identical except selector/preflight.
        # On full CI with cover_s3, the shorter S3 job owns the lint;
        # without cover_s3 Rust does, and PG-only/manual PG still own it.
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())
        manual_rust = job_sections((WORKFLOWS / "rust-demo.yml").read_text())["demo"]
        manual_cover = job_sections((WORKFLOWS / "tor-cover-s3.yml").read_text())["s3-cover-build"]
        manual_pg = job_sections((WORKFLOWS / "tor-pg-runtime.yml").read_text())["postgres-runtime"]
        clippy = "cargo clippy -p tor-web --all-targets --features 's3-covers,s3-paid,s3-free,s3-chapter-import' -- -D warnings"
        rust_gate = """${{ github.workflow == 'TOR integration CI' && needs.select.result == 'success' && fromJSON(needs.select.outputs.suites || '{"postgres":false}').postgres && !fromJSON(needs.select.outputs.suites || '{"cover_s3":false}').cover_s3 && matrix.lane == 'tests' }}"""
        cover_gate = """${{ github.workflow == 'TOR integration CI' && needs.select.result == 'success' && fromJSON(needs.select.outputs.suites || '{"rust":false}').rust && fromJSON(needs.select.outputs.suites || '{"postgres":false}').postgres }}"""
        pg_gate = """${{ (github.workflow != 'TOR integration CI' || needs.select.result != 'success' || !fromJSON(needs.select.outputs.suites || '{"rust":false}').rust) && matrix.shard == 'foundation' }}"""
        for job in (required["demo"], manual_rust):
            self.assertEqual(job.count(clippy), 1)
            self.assertIn("if: " + rust_gate + "\n        run: " + clippy, job)
        for job in (required["s3-cover-build"], manual_cover):
            self.assertEqual(job.count(clippy), 1)
            self.assertIn("if: " + cover_gate + "\n        run: " + clippy, job)
        for job in (required["postgres-runtime"], manual_pg):
            self.assertEqual(job.count(clippy), 1)
            self.assertIn("if: " + pg_gate + "\n        run: " + clippy, job)

        # For all successful selection combinations, PG brings exactly
        # one feature lint owner; for failed selection PG is the owner.
        import itertools
        for rust_selected, pg_selected, cover_selected in itertools.product(
                (False, True), repeat=3):
            owners = (
                int(rust_selected and pg_selected and not cover_selected)
                + int(cover_selected and rust_selected and pg_selected)
                + int(pg_selected and not rust_selected)
            )
            self.assertEqual(owners, int(pg_selected))
        self.assertIn("needs.select.result == 'success'", rust_gate)
        self.assertIn("needs.select.result == 'success'", cover_gate)
        self.assertIn("needs.select.result != 'success'", pg_gate)

    def test_line_a_account_targets_share_cargo_without_losing_tests(self):
        joined = (
            "cargo test -p tor-accounts "
            "--test access_audit_pg --test auth_observation_pg "
            "--test credentials_pg --test slices_pg "
            "-- --ignored --test-threads=1 --show-output"
        )
        for filename in (CI_WORKFLOW, "tor-pg-runtime.yml"):
            with self.subTest(workflow=filename):
                pg = job_sections((WORKFLOWS / filename).read_text())["postgres-runtime"]
                line_a = pg.split(
                    "      - name: Line A credential and login audit PostgreSQL checks", 1
                )[1].split("      - name: D-04 diagnostic HTTP logs", 1)[0]
                self.assertEqual(line_a.count("cargo test -p tor-accounts"), 1)
                self.assertIn(joined, line_a)
                self.assertIn("cargo test -p tor-web --test password_change_http_pg a_07_", line_a)
                self.assertIn("cargo test -p tor-web --test account_sessions_http_pg a_03_", line_a)

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

    def test_flow_visits_reaches_real_pg_and_browser_in_required_ci(self):
        # Required CI and the manual analytics lane both exercise the exact
        # FLOW visits targets. The old sequential PG lane must not duplicate them.
        commands = (
            "cargo test -p tor-recommendations --test flow_visits_pg -- --ignored --test-threads=1",
            "cargo test -p tor-web --test flow_visits_http_pg -- --ignored --test-threads=1",
        )
        for workflow in ("tor-integration.yml", "tor-pg-reading-analytics.yml",
                         "rec-feedback-acceptance.yml"):
            text = expand_ci_actions((WORKFLOWS / workflow).read_text())
            for command in commands:
                with self.subTest(workflow=workflow, command=command):
                    self.assertIn(command, text)
        runtime = (WORKFLOWS / "tor-pg-runtime.yml").read_text()
        for command in commands:
            self.assertNotIn(command, runtime)
        browser = (ROOT / "tools/rec13_browser_acceptance.sh").read_text()
        self.assertIn("TOR_FLOW_VISITS_ENABLED=1", browser)
        self.assertIn("node tools/flow_visits_pg_browser.cjs", browser)

    def test_rec13_acceptance_tests_committed_source_without_rewriting(self):
        self.assertEqual(ACCEPTANCE_WORKFLOWS, {
            "rec-feedback-acceptance.yml",
            "tor-fnd06-two-release.yml",
        })
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
            "tools/ci_prepare_isolated_pg.sh",
            "tools/ci_prepare_pg_client.sh",
            "tools/ci_prepare_chromium.sh",
            "tools/ci_prepare_legacy_analytics.sh",
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

    def test_fnd06_two_release_acceptance_isolated_and_uses_real_distinct_binaries(self):
        text = (WORKFLOWS / "tor-fnd06-two-release.yml").read_text()
        for required in (
            "  pull_request:",
            "  push:",
            "  workflow_dispatch:",
            "    branches: [main]",
            "contents: read",
            "image: postgres:18",
            "TOR_S061_PREVIOUS_SHA:",
            "TOR_S061_PREVIOUS_BINARY:",
            "TOR_S061_S3_ENDPOINT: http://127.0.0.1:9012",
            "git merge-base --is-ancestor",
            "git worktree add --detach",
            "! cmp -s",
            "bash tools/s061_start_minio.sh",
            "python3 tools/s061_cutover_pg_smoke.py",
            'assert report["distinct_binary_validation"] is True',
            'assert 202998431705 in report["added_migrations"]',
            'assert report["same_binary_both_slots"] is False',
            'assert report["populated_catalog_verified"] is True',
            'assert report["postcutover_rollback"] is True',
        ):
            self.assertIn(required, text)
        for forbidden in (
            "contents: write", "pull_request_target:", "secrets.",
            "ssh ", "sudo systemctl", "TOR_S061_S3_ENDPOINT: https://",
        ):
            self.assertNotIn(forbidden, text)

    def test_nightly_full_acceptance_is_scheduled_and_never_deploys(self):
        nightly = (WORKFLOWS / "tor-nightly.yml").read_text()
        self.assertIn("  schedule:", nightly)
        self.assertIn("cron: '17 3 * * *'", nightly)
        self.assertIn("  workflow_dispatch:", nightly)
        self.assertIn("uses: ./.github/workflows/tor-integration.yml", nightly)
        self.assertNotIn("secrets:", nightly)
        self.assertNotIn("tor-one-click-release.yml", nightly)
        integration = (WORKFLOWS / CI_WORKFLOW).read_text()
        self.assertIn("  workflow_call:", integration)

    def test_parallel_postgres_and_rust_shards_preserve_full_test_inventory(self):
        required = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["postgres-runtime"]
        manual = job_sections((WORKFLOWS / "tor-pg-runtime.yml").read_text())["postgres-runtime"]
        for job in (required, manual):
            self.assertIn("shard: [foundation, reading, commerce]", job)
            self.assertIn("CI_PG_SHARD:", job)
            for shard in ("foundation", "reading", "commerce"):
                self.assertIn('if [[ "$CI_PG_SHARD" == "' + shard + '" ]]; then', job)
            self.assertIn("matrix.shard == 'foundation'", job)
            self.assertEqual(job.count('ci_pg_phase_done "'), 6)
        demo = job_sections((WORKFLOWS / CI_WORKFLOW).read_text())["demo"]
        self.assertIn("lane: [tests, release]", demo)
        self.assertIn("matrix.lane == 'tests'", demo)
        self.assertIn("matrix.lane == 'release'", demo)

    def test_doppelscan_audit_is_isolated_and_advisory(self):
        text = (WORKFLOWS / "reuse-audit.yml").read_text()
        self.assertEqual(set(job_sections(text)), {"scan"})
        for required in (
            "  workflow_dispatch:", "  pull_request:", "contents: read",
            "continue-on-error: true",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "persist-credentials: false", '--network none --read-only',
            "--cap-drop ALL --security-opt no-new-privileges",
            "--cpus 2 --memory 4g", ":/src:ro", ":/tools:ro",
            "sha256sum -c -", "doppelscan-v0.9.0-x86_64-unknown-linux-gnu.tar.gz",
            "e5d5abd63abfb74dddfc52d60d58144f402218aa49b3b6b6e610d1e3171f355f",
            "git archive", "github.sha", "--only rust --min-tokens 50",
            "--similarity 0.8", "summary.json",
            "run: bash tools/reuse_doppelscan_delta.sh", "BASE_SHA:",
        ):
            self.assertIn(required, text)
        for forbidden in ("contents: write", "pull_request_target:",
                          "secrets.", "secrets:", "  push:", "ssh ",
                          "cargo install", "jscpd", "cargo-dupes"):
            self.assertNotIn(forbidden, text)
        delta = (ROOT / "tools/reuse_doppelscan_delta.sh").read_text()
        for required in ("git merge-base --is-ancestor", "git archive",
                         "baseline /work/src", "check /work/src",
                         "--network none --read-only", ":/work/src:ro",
                         "delta-summary.json", "0|1)"):
            self.assertIn(required, delta)
        self.assertNotIn("reuse-audit.yml",
                         (WORKFLOWS / CI_WORKFLOW).read_text())

    def test_rustsec_is_independent_read_only_and_fails_closed(self):
        text = (WORKFLOWS / "rustsec.yml").read_text()
        self.assertEqual(set(job_sections(text)), {"audit"})
        for required in (
            "  pull_request:", "  schedule:", "  workflow_dispatch:",
            "cron: '17 4 * * *'", "'**/Cargo.toml'", "'**/Cargo.lock'",
            "contents: read", "persist-credentials: false",
            "git ls-files --error-unmatch Cargo.lock",
            "cargo metadata --locked --all-features",
            'cargo install --locked --version "=$AUDIT_VERSION" cargo-audit',
            'cargo install --locked --version "=$DENY_VERSION" cargo-deny',
            "cargo audit --json", "cargo deny --locked --all-features",
            'cmp Cargo.lock "$RUNNER_TEMP/rustsec-report/Cargo.lock"',
            "git diff --exit-code -- Cargo.lock",
            "exit \"$rc\"", "always() && !cancelled()",
        ):
            self.assertIn(required, text)
        self.assertEqual(text.count('exit "$rc"'), 2)
        for forbidden in (
            "contents: write", "pull_request_target:", "secrets.",
            "continue-on-error:", "  push:", "cargo generate-lockfile",
            "ssh ", "sudo systemctl", "--ignore",
        ):
            self.assertNotIn(forbidden, text)

    def test_no_unknown_workflows_or_missing_harness(self):
        known = ({filename for filename, _ in SUITES.values()}
                 | {CI_WORKFLOW, "rules.yml", "ci-selection.yml", "tor-nightly.yml",
                    "reuse-audit.yml", "rustsec.yml"}
                 | MANUAL_WORKFLOWS | BUILD_ONLY_WORKFLOWS | PERFORMANCE_WORKFLOWS | ISOLATED_GATEWAY_WORKFLOWS | ACCEPTANCE_WORKFLOWS)
        self.assertEqual({p.name for p in WORKFLOWS.glob("*.yml")}, known)
        self.assertEqual(set(SUITES), set(SUITE_JOB_IDS))
        self.assertEqual(len(set(SUITE_JOB_IDS.values())), len(SUITES))
        for path, suites in TEST_SUITES.items():
            with self.subTest(path=path):
                self.assertTrue((ROOT / path).is_file())
                self.assertTrue(suites <= SUITES.keys())


if __name__ == "__main__":
    unittest.main()
