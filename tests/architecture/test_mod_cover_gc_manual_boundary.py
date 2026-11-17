"""#667: private S3 cleanup coordinator must not run on HTTP startup."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
OPS = (ROOT / "crates/web/src/cover_private_gc_ops.rs").read_text(encoding="utf-8")
MAIN = (ROOT / "crates/web/src/main.rs").read_text(encoding="utf-8")
WEB = (ROOT / "crates/web/src/lib.rs").read_text(encoding="utf-8")
S3_TEST = (ROOT / "crates/web/tests/cover_s3_pg/source_cleanup.rs").read_text(encoding="utf-8")


class PrivateGcManualBoundary(unittest.TestCase):
    def test_not_activated_from_server_startup_or_routes(self):
        self.assertIn("pub mod cover_private_gc_ops;", WEB)
        self.assertNotIn("cover_private_gc_ops", MAIN)
        self.assertNotIn("run_single_bounded_batch", MAIN)
        self.assertIn("run_single_bounded_batch", OPS)
        self.assertNotIn("tokio::spawn", OPS)
        self.assertNotIn("interval(", OPS)
        self.assertNotIn(".route(", OPS)

    def test_ordered_pass_is_fail_closed_on_variant_deferred(self):
        self.assertIn("variants.deferred == 0", OPS)
        self.assertIn("let variants = self.variants.run_batch().await?;", OPS)
        self.assertIn("Some(self.sources.run_batch().await?)", OPS)
        self.assertLess(
            OPS.index("let variants = self.variants.run_batch().await?;"),
            OPS.index("Some(self.sources.run_batch().await?)"),
        )
        self.assertIn("PrivateCoverGcCoordinator::new", S3_TEST)
        self.assertIn("first_cycle.sources.unwrap().claimed, 0", S3_TEST)
        self.assertIn("another recipe is still pending", S3_TEST)


if __name__ == "__main__":
    unittest.main()
