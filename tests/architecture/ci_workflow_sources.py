"""Expand the one reviewed local CI action for existing architecture assertions.

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
