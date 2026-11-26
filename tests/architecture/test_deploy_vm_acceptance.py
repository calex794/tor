"""Independent hosted VM lab has no operational authority."""
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import ISOLATED_GATEWAY_WORKFLOWS


class DeploymentVMTests(unittest.TestCase):
    def test_disposable_workflow_has_no_production_access(self):
        name = "tor-deploy-vm.yml"
        self.assertIn(name, ISOLATED_GATEWAY_WORKFLOWS)
        workflow = (ROOT / ".github/workflows" / name).read_text()
        for required in ("runs-on: ubuntu-24.04", "contents: read", "persist-credentials: false",
                         "  pull_request:", "  workflow_dispatch:", "RUNNER_ENVIRONMENT",
                         "hosted_vm_setup.sh", "hosted_vm_acceptance.py",
                         "grep -Fxq 'DEPLOY08A_VM_ACCEPTANCE_OK'"):
            self.assertIn(required, workflow)
        for forbidden in ("secrets.", "environment: tor-staging", "  push:",
                          "pull_request_target:", "self-hosted\n", "scp ", "ssh ",
                          "continue-on-error:", "contents: write"):
            self.assertNotIn(forbidden, workflow)

    def test_pg_setup_checks_real_parser_before_unit_start(self):
        setup = (ROOT / "tests/deploy/hosted_vm_setup.sh").read_text()
        for setting, value in (("listen_addresses", "127.0.0.1"),
                               ("password_encryption", "scram-sha-256"),
                               ("shared_buffers", "64MB")):
            self.assertIn(f"pg_conftool 18 main set {setting} " + '"' + "'" + value + "'" + '"', setup)
        self.assertIn('-c config_file=/etc/postgresql/18/main/postgresql.conf -C "$1"', setup)
        self.assertLess(setup.index("DEPLOY08A_PG_CONFIG_PARSED_OK"),
                        setup.index("if ! systemctl start postgresql@18-main.service"))
        self.assertLess(setup.index("if ! systemctl start postgresql@18-main.service"),
                        setup.index("DEPLOY08A_REAL_PG18_ACTIVE_OK"))

    def test_fixture_syntax_and_real_boundaries(self):
        setup = ROOT / "tests/deploy/hosted_vm_setup.sh"
        subprocess.run(["bash", "-n", str(setup)], check=True)
        lab = ROOT / "tests/deploy/hosted_vm_acceptance.py"
        text = lab.read_text()
        compile(text, str(lab), "exec")
        for required in ("first.run(", "first.Operations(", "edge.ProductionNginx()",
                         "unit.Systemd()", "coordinated.Coordinator(",
                         '["bash", str(target / "apply-tor-migrations-vps.sh"), sha]',
                         'executor.command("systemctl", "restart", executor.APP',
                         "os.fork()", "os._exit(73)", "coordinator().recover()",
                         "database_witness() == db", "secrets_witness() == credentials"):
            self.assertIn(required, text)
        self.assertNotIn("curl -k", text)
        self.assertIn("github-hosted", setup.read_text())
        self.assertNotIn("pg_dropcluster", setup.read_text())


if __name__ == "__main__":
    unittest.main()
