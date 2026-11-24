"""Admin namespace migration inventory and final regression guard.

The failing assertion lists paths only: no source lines, tokens or config.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
OLD_PUBLIC_PREFIX = re.compile(r"/admin(?=/|-|[\"'\x60\s?#)]|$)", re.I)
TEXT = {".rs", ".py", ".cjs", ".js", ".mjs", ".html", ".json", ".css",
        ".sh", ".ts", ".tsx", ".md", ".sql", ".toml", ".tsv"}
RUNTIME = ("crates/web/src", "crates/web/assets", "crates/web/templates",
           "crates/route_intelligence/src")
OTHER = ("tools", "crates/web/tests", "tests/architecture",
         "docs/contracts", ".github/workflows")


def inventory(paths):
    found = []
    for top in paths:
        base = ROOT / top
        if not base.exists():
            continue
        for path in sorted(base.rglob("*")):
            if path.is_symlink() or not path.is_file() or path.suffix not in TEXT:
                continue
            if path.name == "test_admin_namespace_cutover.py":
                continue
            try:
                content = path.read_text(encoding="utf-8")
            except (UnicodeError, OSError):
                continue
            count = len(OLD_PUBLIC_PREFIX.findall(content))
            if count:
                found.append((path.relative_to(ROOT).as_posix(), count))
    return found


class AdminNamespaceCutoverTests(unittest.TestCase):
    def test_no_live_admin_routes_or_links_with_old_prefix(self):
        old = inventory(RUNTIME)
        other = [(path, count) for path, count in inventory(OTHER)
                 if not path.endswith(".md")
                 and path != "tools/a09_honeypot_browser.cjs"]
        summary = [f"ADMIN_NAMESPACE_{scope} {path} {n}"
                   for scope, items in (("LIVE", old), ("OTHER", other))
                   for path, n in items]
        self.assertEqual(old + other, [], "\n" + "\n".join(summary[:280]))

    def test_unrelated_honeypot_name_is_intentionally_preserved(self):
        decoy = (ROOT / "tools/a09_honeypot_browser.cjs").read_text(encoding="utf-8")
        self.assertIn("__bp_honeypot/admin/private", decoy)


if __name__ == "__main__":
    unittest.main()
