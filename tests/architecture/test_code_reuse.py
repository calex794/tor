"""REUSE-01: fixtures and an actual committed-checkout report in Repository rules."""
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from audit_code_reuse import Source, audit, audit_git, git  # noqa: E402


def source(path, body):
    raw = body.encode("utf-8")
    return Source.from_bytes(path, hashlib.sha256(raw).hexdigest(), raw)


BLOCK = "\n".join(f"let value_{i} = validate_request(input_{i})?;" for i in range(10))


class ReuseAuditTests(unittest.TestCase):
    def test_exact_files_and_shifted_block(self):
        a = source("crates/web/src/a.rs", BLOCK)
        b = source("crates/web/src/b.rs", BLOCK)
        c = source("crates/books/src/c.rs", "// header\n\n" + BLOCK + "\nfinish();")
        result = audit([c, a, b])
        self.assertEqual(result["byte_identical_file_groups"], [[a.path, b.path]])
        self.assertEqual(result["candidate_pairs"], 3)
        self.assertEqual(result["candidate_lines"], 30)
        self.assertEqual(result["by_area"]["crates/web"]["candidate_lines"], 20)
        self.assertTrue(any(item["left"]["start_line"] == 3
                            for item in result["candidates"]))
        self.assertNotIn("validate_request", json.dumps(result))
        self.assertEqual(result, audit([a, b, c]))
        self.assertEqual(audit([a, b, c], top=0)["candidate_lines"], 30)

    def test_same_file_nonoverlap_and_scope(self):
        a = source("crates/web/tests/a.rs", BLOCK + "\nseparator();\n" + BLOCK)
        result = audit([a])
        self.assertEqual(result["candidate_pairs"], 1)
        self.assertEqual(result["by_scope"]["test"]["candidate_lines"], 20)
        hit = result["candidates"][0]
        self.assertLess(hit["left"]["end_line"], hit["right"]["start_line"])

    def test_no_cross_language_or_too_short_match(self):
        result = audit([source("a.rs", BLOCK), source("a.py", BLOCK),
                        source("short.rs", "let x = 1;")])
        self.assertEqual(result["candidate_pairs"], 0)
        self.assertEqual(result["candidate_lines"], 0)

    def test_common_windows_are_counted_as_skipped(self):
        result = audit([source(f"{i}.rs", BLOCK) for i in range(4)],
                       occurrence_cap=2)
        self.assertGreater(result["high_frequency_windows_skipped"], 0)
        self.assertEqual(result["candidate_lines"], 0)
        self.assertEqual(len(result["byte_identical_file_groups"][0]), 4)

    def test_committed_tree_ignores_dirty_and_untracked_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            git(root, "init", "-q")
            for path in ("a.rs", "b.rs"):
                (root / path).write_text(BLOCK, encoding="utf-8")
            git(root, "add", ".")
            git(root, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "-qm", "fixture")
            first = audit_git(root)
            (root / "a.rs").write_text("changed", encoding="utf-8")
            (root / "untracked.rs").write_text(BLOCK, encoding="utf-8")
            self.assertEqual(first, audit_git(root))
            self.assertEqual(first["files"], 2)
            self.assertEqual(first["candidate_lines"], 20)

    def test_repository_checkout_report(self):
        # Runs via existing Repository rules unittest discovery. The score is
        # observational, not a threshold or a blocker for pre-existing clones.
        result = audit_git(ROOT, top=30)
        self.assertGreater(result["files"], 0)
        self.assertEqual(result["schema"], "tor.reuse.audit.v1")
        self.assertLessEqual(result["candidate_lines"], result["nonblank_lines"])
        print("REUSE-01 committed checkout report: " + json.dumps(
            result, ensure_ascii=True, sort_keys=True), flush=True)


if __name__ == "__main__":
    unittest.main()
