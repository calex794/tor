"""AB-12: current TOR binary must install scanner filtering after route merge."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ScannerGuardWiringTests(unittest.TestCase):
    def test_security_guard_is_not_only_a_future_nginx_example(self):
        main = (ROOT / "crates/web/src/main.rs").read_text()
        module = (ROOT / "crates/web/src/scanner_guard.rs").read_text()
        assert "let app = tor_web::scanner_guard::protect(app);" in main
        assert main.index("scanner_guard::protect(app)") > main.index("start_managed(app")
        assert main.index("scanner_guard::protect(app)") < main.index("flow_runtime::serve(listener")
        self.assertIn("if !scanner_path(request.uri().path())", module)
        self.assertIn("next.run(request).await", module)
        self.assertIn("StatusCode::NOT_FOUND", module)
        self.assertNotIn("x-forwarded-for", module.split("#[cfg(test)]", 1)[0].lower())
        self.assertNotIn("remote_addr", module.split("#[cfg(test)]", 1)[0].lower())

    def test_guard_exposes_only_aggregate_metric_without_dynamic_labels(self):
        observability = (ROOT / "crates/web/src/observability.rs").read_text()
        self.assertIn("tor_security_scanner_rejected_total", observability)
        self.assertIn("scanner_guard::rejected_total()", observability)
        self.assertNotIn("scanner_guard::rejected_total(ip", observability)


if __name__ == "__main__":
    unittest.main()
