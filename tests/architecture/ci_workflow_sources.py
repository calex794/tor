"""Expand reviewed local CI actions for existing architecture assertions.

This is a test-only source view, not a YAML executor. The explicit shell and
strict step shape are checked before reconstructing the original job body.
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
ACTION_PATH = ROOT / ".github/actions/pg-reading-analytics/action.yml"
ACTION_REF = "        uses: ./.github/actions/pg-reading-analytics\n"
ACTION_CALL = ("      - name: Run shared PostgreSQL reading analytics acceptance\n"
               + ACTION_REF)
ACTION_HEADER = (
    "name: PostgreSQL reading analytics acceptance\n"
    "description: Shared isolated PostgreSQL analytics checks for required and manual CI\n"
    "runs:\n  using: composite\n  steps:\n"
)
SHELL = "      shell: bash -e {0}\n"
STEP_NAMES = (
    "Feature boundary checks",
    "Prepare isolated database",
    "Apply real SQLx migrations twice",
    "Enable bounded lock diagnostics only on disposable analytics DB",
    "Prepare explicit legacy and installed-upgrade analytics databases",
    "Installed 190 to current analytics retirement with canonical and preference preservation",
    "Reader audience, FLOW and SEO PostgreSQL acceptance",
)


def analytics_steps(action):
    if not action.startswith(ACTION_HEADER):
        raise ValueError("unexpected local analytics action metadata")
    chunks = re.split(r"(?m)(?=^    - name: )", action[len(ACTION_HEADER):])
    if chunks[0] or len(chunks) != len(STEP_NAMES) + 1:
        raise ValueError("unexpected local analytics step count")
    result = []
    for name, chunk in zip(STEP_NAMES, chunks[1:]):
        lines = chunk.splitlines(keepends=True)
        if (len(lines) < 3 or lines[0] != f"    - name: {name}\n"
                or lines[1] != SHELL or not lines[2].startswith("      run: ")):
            raise ValueError("unexpected local analytics step shape")
        if lines[2] == "      run: |\n":
            if len(lines) < 4 or any(not line.startswith("        ") for line in lines[3:]):
                raise ValueError("invalid local analytics run block")
        elif len(lines) != 3:
            raise ValueError("unexpected local analytics step fields")
        result.extend("  " + line for line in (lines[0], *lines[2:]))
    return "".join(result)


def expand_analytics(text, action=None):
    """Keep job metadata and gates intact; resolve only the exact local call."""
    if ACTION_REF not in text:
        return text
    if text.count(ACTION_REF) != 1 or text.count(ACTION_CALL) != 1:
        raise ValueError("unexpected local analytics action call")
    if action is None:
        action = ACTION_PATH.read_text(encoding="utf-8")
    return text.replace(ACTION_CALL, analytics_steps(action), 1)


REC_ACTION_PATH = ROOT / ".github/actions/pg-rec-browser/action.yml"
REC_ACTION_REF = "        uses: ./.github/actions/pg-rec-browser\n"
REC_ACTION_CALL = ("      - name: Run shared PostgreSQL REC13 browser acceptance\n"
                   + REC_ACTION_REF + "        with:\n          lane: ${{ matrix.lane }}\n")
REC_ACTION_HEADER = (
    "name: PostgreSQL REC13 browser acceptance\n"
    "description: Shared isolated current and historical REC browser checks\n"
    "inputs:\n"
    "  lane:\n"
    "    description: Current or historical test lane\n"
    "    required: true\n"
    "runs:\n"
    "  using: composite\n"
    "  steps:\n"
)
REC_LANE_GUARD = (
    "    - name: Validate REC matrix lane\n"
    "      shell: bash -e {0}\n"
    "      env:\n"
    "        TOR_REC13_LANE: ${{ inputs.lane }}\n"
    "      run: |\n"
    "        case \"$TOR_REC13_LANE\" in\n"
    "          current|historical) ;;\n"
    "          *) echo 'Unsupported REC matrix lane' >&2; exit 2 ;;\n"
    "        esac\n"
)
REC_STEP_NAMES = (
    "Feature boundary checks",
    "Prepare isolated database",
    "Apply real SQLx migrations twice",
    "Prepare explicit legacy and installed-upgrade analytics databases",
    "Installed 190 to current analytics retirement with canonical and preference preservation",
    "REC13 real Chromium and historical HTTP acceptance",
    "100k catalogue contextual model PostgreSQL acceptance",
)


def rec_browser_steps(action):
    prefix = REC_ACTION_HEADER + REC_LANE_GUARD
    if not action.startswith(prefix):
        raise ValueError("unexpected REC action metadata or lane guard")
    chunks = re.split(r"(?m)(?=^    - name: )", action[len(prefix):])
    if chunks[0] or len(chunks) != len(REC_STEP_NAMES) + 1:
        raise ValueError("unexpected REC step count")
    result = []
    for index, (name, chunk) in enumerate(zip(REC_STEP_NAMES, chunks[1:])):
        lines = chunk.splitlines(keepends=True)
        fields = [f"    - name: {name}\n", SHELL]
        if index in (3, 4, 6):
            lane = "current" if index == 6 else "historical"
            fields.append("      if: ${{ inputs.lane == '" + lane + "' }}\n")
        if index == 5:
            fields.extend(("      env:\n", "        TOR_REC13_LANE: ${{ inputs.lane }}\n"))
        n = len(fields)
        if (lines[:n] != fields or len(lines) <= n
                or not lines[n].startswith("      run: ")):
            raise ValueError("unexpected REC step shape or lane gate")
        if lines[n] == "      run: |\n":
            if len(lines) <= n + 1 or any(not line.startswith("        ") for line in lines[n + 1:]):
                raise ValueError("invalid REC run block")
        elif len(lines) != n + 1:
            raise ValueError("unexpected REC step fields")
        result.extend("  " + line.replace("inputs.lane", "matrix.lane")
                      for line in (lines[0], *lines[2:]))
    return "".join(result)


def expand_rec_browser(text, action=None):
    if REC_ACTION_REF not in text:
        return text
    if text.count(REC_ACTION_REF) != 1 or text.count(REC_ACTION_CALL) != 1:
        raise ValueError("unexpected REC action call or lane mapping")
    if action is None:
        action = REC_ACTION_PATH.read_text(encoding="utf-8")
    return text.replace(REC_ACTION_CALL, rec_browser_steps(action), 1)


COVER_ACTION_PATH = ROOT / ".github/actions/pg-cover-browser/action.yml"
COVER_ACTION_REF = "        uses: ./.github/actions/pg-cover-browser\n"
COVER_ACTION_CALL = ("      - name: Run shared PostgreSQL cover browser acceptance\n"
                     + COVER_ACTION_REF)
COVER_ACTION_HEADER = (
    "name: PostgreSQL cover browser acceptance\n"
    "description: Shared isolated owner account and private S3 cover browser checks\n"
    "runs:\n"
    "  using: composite\n"
    "  steps:\n"
)
COVER_STEP_NAMES = (
    "Create disposable PostgreSQL roles and apply canonical migrations",
    "Start checksum-verified MinIO on loopback",
    "Prepare isolated reset test mailbox",
    "Create private test bucket using existing verified S3 provider test",
    "Build real Rust server with private S3 enabled",
    "Install Chromium",
    "Real owner session, S3 upload, PostgreSQL worker, browser layout and public WebP",
)
COVER_BROWSER_ENV = (
    "      env:\n"
    "        TOR_COVER_S3_BUCKET: tor-private-cover-ci\n"
    "        TOR_COVER_WORKER_ENABLED: '1'\n"
    "        TOR_COVER_EXPECT_LOSSY_WEBP: '1'\n"
    "        TOR_AUTH_ENABLED: '1'\n"
    "        TOR_PUBLIC_ORIGIN: http://127.0.0.1:3098\n"
    "        TOR_LISTEN_ADDR: 127.0.0.1:3098\n"
    "        DATABASE_URL: ${{ env.APP_URL }}\n"
    "        AWS_REGION: us-east-1\n"
    "        AWS_ACCESS_KEY_ID: tor-cover-ci-user\n"
    "        AWS_SECRET_ACCESS_KEY: tor-cover-ci-secret-only\n"
    "        AWS_ENDPOINT_URL_S3: http://127.0.0.1:9000\n"
    "        AWS_EC2_METADATA_DISABLED: 'true'\n"
    "        TOR_COVER_BROWSER_SHOTS: ${{ runner.temp }}/cover-browser-shots\n"
    "        TOR_PASSWORD_RESET_ENABLED: '1'\n"
    "        TOR_RESET_SENDMAIL: ${{ runner.temp }}/account-reset-sendmail\n"
    "        TOR_RESET_MAIL_FROM: no-reply@example.invalid\n"
    "        TOR_RESET_TEST_MAIL_DIR: ${{ runner.temp }}/account-reset-mail\n"
)


def cover_browser_steps(action):
    if not action.startswith(COVER_ACTION_HEADER):
        raise ValueError("unexpected cover browser action metadata")
    chunks = re.split(r"(?m)(?=^    - name: )", action[len(COVER_ACTION_HEADER):])
    if chunks[0] or len(chunks) != len(COVER_STEP_NAMES) + 1:
        raise ValueError("unexpected cover browser step count")
    result = []
    for index, (name, chunk) in enumerate(zip(COVER_STEP_NAMES, chunks[1:])):
        lines = chunk.splitlines(keepends=True)
        fields = [f"    - name: {name}\n", SHELL]
        if index == len(COVER_STEP_NAMES) - 1:
            fields.extend(COVER_BROWSER_ENV.splitlines(keepends=True))
        n = len(fields)
        if (lines[:n] != fields or len(lines) <= n
                or not lines[n].startswith("      run: ")):
            raise ValueError("unexpected cover browser step shape or environment")
        if lines[n] == "      run: |\n":
            if len(lines) <= n + 1 or any(not line.startswith("        ") for line in lines[n + 1:]):
                raise ValueError("invalid cover browser run block")
        elif len(lines) != n + 1:
            raise ValueError("unexpected cover browser step fields")
        result.extend("  " + line for line in (lines[0], *lines[2:]))
    return "".join(result)


def expand_cover_browser(text, action=None):
    if COVER_ACTION_REF not in text:
        return text
    if text.count(COVER_ACTION_REF) != 1 or text.count(COVER_ACTION_CALL) != 1:
        raise ValueError("unexpected cover browser action call")
    if action is None:
        action = COVER_ACTION_PATH.read_text(encoding="utf-8")
    return text.replace(COVER_ACTION_CALL, cover_browser_steps(action), 1)


S3_COVER_ACTION_PATH = ROOT / ".github/actions/pg-cover-s3/action.yml"
S3_COVER_ACTION_REF = "        uses: ./.github/actions/pg-cover-s3\n"
S3_COVER_ACTION_CALL = ("      - name: Run shared private S3 cover acceptance\n"
                        + S3_COVER_ACTION_REF)
S3_COVER_ACTION_HEADER = (
    "name: Private S3 cover acceptance\n"
    "description: Shared isolated MinIO and PostgreSQL cover checks\n"
    "runs:\n"
    "  using: composite\n"
    "  steps:\n"
)
S3_COVER_STEP_NAMES = (
    "Compile private S3 cover adapter without provider credentials",
    "Start upstream release of MinIO on loopback with SHA-256 verification",
    "Test signed S3 PUT HEAD GET, integrity and strict size bounds",
    "Prepare isolated PostgreSQL schema and owner 950 for OBS-09",
    "Run signed MinIO and restricted PG whole-cover user scenario",
    "MOD-COVER private GC read-only preview (disposable PG, no S3 deletes)",
    "Read-only cover-size inventory against disposable PG",
    "Deny anonymous S3 access to actual PG-attached WebP cover",
    "Assert direct anonymous URL cannot read the private cover",
)
S3_COVER_MINIO_ENV = (
    "      env:\n"
    "        TOR_TEST_S3_ENDPOINT: http://127.0.0.1:9000\n"
    "        TOR_TEST_S3_ACCESS_KEY: tor-cover-ci-user\n"
    "        TOR_TEST_S3_SECRET_KEY: tor-cover-ci-secret-only\n"
)
S3_COVER_READONLY_ENV = (
    "      env:\n"
    "        PGUSER: tor_app\n"
    "        PGPASSWORD: tor-obs09-app\n"
    "        PGDATABASE: tor_ci\n"
)


def s3_cover_steps(action):
    if not action.startswith(S3_COVER_ACTION_HEADER):
        raise ValueError("unexpected S3 cover action metadata")
    chunks = re.split(r"(?m)(?=^    - name: )", action[len(S3_COVER_ACTION_HEADER):])
    if chunks[0] or len(chunks) != len(S3_COVER_STEP_NAMES) + 1:
        raise ValueError("unexpected S3 cover step count")
    result = []
    for index, (name, chunk) in enumerate(zip(S3_COVER_STEP_NAMES, chunks[1:])):
        lines = chunk.splitlines(keepends=True)
        fields = [f"    - name: {name}\n", SHELL]
        if index in (2, 4):
            fields.extend(S3_COVER_MINIO_ENV.splitlines(keepends=True))
        if index in (5, 6):
            fields.extend(S3_COVER_READONLY_ENV.splitlines(keepends=True))
        n = len(fields)
        if (lines[:n] != fields or len(lines) <= n
                or not lines[n].startswith("      run: ")):
            raise ValueError("unexpected S3 cover step shape or environment")
        if lines[n] == "      run: |\n":
            if len(lines) <= n + 1 or any(not line.startswith("        ") for line in lines[n + 1:]):
                raise ValueError("invalid S3 cover run block")
        elif len(lines) != n + 1:
            raise ValueError("unexpected S3 cover step fields")
        result.extend("  " + line for line in (lines[0], *lines[2:]))
    return "".join(result)


def expand_s3_cover(text, action=None):
    if S3_COVER_ACTION_REF not in text:
        return text
    if text.count(S3_COVER_ACTION_REF) != 1 or text.count(S3_COVER_ACTION_CALL) != 1:
        raise ValueError("unexpected S3 cover action call")
    if action is None:
        action = S3_COVER_ACTION_PATH.read_text(encoding="utf-8")
    return text.replace(S3_COVER_ACTION_CALL, s3_cover_steps(action), 1)


PG_RUNTIME_PATH = ROOT / "tools/ci_pg_runtime_acceptance.sh"
PG_RUNTIME_STEP = "      - name: Database and HTTP integration\n"
PG_RUNTIME_REF = "        run: bash tools/ci_pg_runtime_acceptance.sh\n"
PG_RUNTIME_CALL = PG_RUNTIME_STEP + PG_RUNTIME_REF + "\n"
PG_RUNTIME_PREFIX = (
    "#!/usr/bin/env bash\n"
    "# Shared PostgreSQL runtime acceptance. Preserve the original shell-local\n"
    "# Cargo instrumentation and per-command feature flags in all three shards.\n"
    "set -e\n"
    "case \"${CI_PG_SHARD:-}\" in\n"
    "  foundation|reading|commerce) ;;\n"
    "  *) echo 'Unsupported PostgreSQL CI shard' >&2; exit 2 ;;\n"
    "esac\n"
)


def expand_pg_runtime(text, script=None):
    """Resolve only the exact reviewed PG runtime step for existing assertions."""
    if PG_RUNTIME_REF not in text:
        return text
    if text.count(PG_RUNTIME_REF) != 1 or text.count(PG_RUNTIME_CALL) != 1:
        raise ValueError("unexpected PostgreSQL runtime script call")
    if script is None:
        script = PG_RUNTIME_PATH.read_text(encoding="utf-8")
    if not script.startswith(PG_RUNTIME_PREFIX) or not script.endswith("\n"):
        raise ValueError("missing PostgreSQL runtime shell guard")
    body = script[len(PG_RUNTIME_PREFIX):]
    if not body or "set +e" in body:
        raise ValueError("invalid PostgreSQL runtime body")
    expanded = PG_RUNTIME_STEP + "        run: |\n" + "".join(
        "          " + line for line in body.splitlines(keepends=True)) + "\n"
    return text.replace(PG_RUNTIME_CALL, expanded, 1)


PAID_E2E_SCRIPTS = (
    "tools/ci_paid_e2e_prepare_pg.sh",
    "tools/ci_paid_e2e_start_minio.sh",
    "tools/ci_paid_e2e_acceptance.sh",
)
PAID_E2E_PREFIX = (
    "#!/usr/bin/env bash\n"
    "# CI-only disposable PostgreSQL and private loopback S3 fixtures.\n"
    "set -euo pipefail\n"
    "if [[ \"${GITHUB_ACTIONS:-}\" != true || \"${PGHOST:-}\" != 127.0.0.1 ||\n"
    "      \"${PGDATABASE:-}\" != postgres ||\n"
    "      \"${TEST_DATABASE_URL:-}\" != postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci ||\n"
    "      \"${MIGRATOR_URL:-}\" != postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci ]]; then\n"
    "  echo 'Refusing non-isolated paid E2E fixture' >&2\n"
    "  exit 2\n"
    "fi\n"
)
PAID_E2E_S3_GUARD = (
    "if [[ \"${TOR_TEST_S3_ENDPOINT:-}\" != http://127.0.0.1:9000 ]]; then\n"
    "  echo 'Refusing non-loopback paid E2E S3 endpoint' >&2\n"
    "  exit 2\n"
    "fi\n"
)


def expand_paid_e2e(text):
    """Restore only the three reviewed paid E2E run bodies for source tests."""
    for index, path in enumerate(PAID_E2E_SCRIPTS):
        call = f"        run: bash {path}\n"
        if call not in text:
            continue
        if text.count(call) != 1:
            raise ValueError("ambiguous paid E2E script call")
        script = (ROOT / path).read_text(encoding="utf-8")
        prefix = PAID_E2E_PREFIX + (PAID_E2E_S3_GUARD if index == 2 else "")
        if not script.startswith(prefix) or not script.endswith("\n"):
            raise ValueError("missing paid E2E isolation guard")
        body = script[len(prefix):]
        if not body.startswith("set -euo pipefail\n"):
            raise ValueError("missing paid E2E fail-closed shell")
        inline = "        run: |\n" + "".join(
            "          " + line for line in body.splitlines(keepends=True))
        text = text.replace(call, inline, 1)
    return text


def expand_ci_actions(text):
    return expand_paid_e2e(expand_pg_runtime(expand_s3_cover(expand_cover_browser(
        expand_rec_browser(expand_analytics(text))))))
