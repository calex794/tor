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


def expand_ci_actions(text):
    return expand_rec_browser(expand_analytics(text))
