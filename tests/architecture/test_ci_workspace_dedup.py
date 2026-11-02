"""Keep workspace Rust gates single-owned without weakening PostgreSQL acceptance."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"


class WorkspaceDedupTests(unittest.TestCase):
    def test_rust_suite_owns_automatic_workspace_checks(self):
        rust = (WORKFLOWS / "rust-demo.yml").read_text()
        postgres = (WORKFLOWS / "tor-pg-runtime.yml").read_text()
        for command in (
            "cargo fmt --all -- --check",
            "cargo test --workspace --all-targets",
            "cargo clippy --workspace --all-targets -- -D warnings",
        ):
            with self.subTest(command=command):
                self.assertIn(command, rust)
                self.assertEqual(postgres.count(command), 1)
        self.assertIn(
            "      - name: Standalone manual workspace baseline\n"
            "        if: ${{ github.event_name == 'workflow_dispatch' }}\n"
            "        run: |\n"
            "          cargo fmt --all -- --check\n"
            "          cargo test --workspace --all-targets\n"
            "          cargo clippy --workspace --all-targets -- -D warnings\n",
            postgres,
        )

    def test_postgres_keeps_feature_lints_and_database_acceptance(self):
        postgres = (WORKFLOWS / "tor-pg-runtime.yml").read_text()
        self.assertIn("        image: postgres:18", postgres)
        self.assertIn("run: python3 tools/check_feature_layout.py", postgres)
        self.assertIn(
            "run: cargo clippy -p tor-web --all-targets "
            "--features 's3-covers,s3-paid' -- -D warnings",
            postgres,
        )
        for command in (
            "cargo test -p tor-web --test postgres_http -- --ignored",
            "cargo test -p tor-web --test book_follow_http_pg -- --ignored",
            "cargo test -p tor-storage --features pg --test storage_registry_pg -- --ignored",
        ):
            with self.subTest(command=command):
                self.assertIn(command, postgres)


if __name__ == "__main__":
    unittest.main()
