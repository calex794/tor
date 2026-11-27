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
        block = setup.split("<<'TOR_VM_PG_CONFIG'\n", 1)[1].split("\nTOR_VM_PG_CONFIG\n", 1)[0]
        settings = [line for line in block.splitlines()
                    if line.strip() and not line.startswith("#")]
        self.assertEqual(settings, ["port = 5432", "listen_addresses = '127.0.0.1'",
                                    "password_encryption = 'scram-sha-256'",
                                    "max_connections = 20", "shared_buffers = '64MB'"])
        self.assertNotIn("pg_conftool 18 main set", setup)
        self.assertIn("pg_size_bytes(current_setting('shared_buffers'))", setup)
        self.assertEqual(setup.count("DEPLOY08A_HOSTED_VM_FIXTURE_READY"), 1)
        self.assertEqual(setup.count("useradd --create-home"), 1)
        self.assertIn("grep -Eq '^18[0-9]{4}$'", setup)
        self.assertIn('-c config_file=/etc/postgresql/18/main/postgresql.conf -C "$1"', setup)
        self.assertLess(setup.index("DEPLOY08A_PG_CONFIG_PARSED_OK"),
                        setup.index("if ! systemctl start postgresql@18-main.service"))
        self.assertLess(setup.index("if ! systemctl start postgresql@18-main.service"),
                        setup.index("DEPLOY08A_REAL_PG18_ACTIVE_OK"))

    def test_runner_opt_normalization_is_fixture_only(self):
        setup = (ROOT / "tests/deploy/hosted_vm_setup.sh").read_text()
        self.assertIn('[[ -d /opt && ! -L /opt && $(readlink -f /opt) == /opt ]]', setup)
        self.assertLess(setup.index("REFUSE: disposable GitHub-hosted VM only"),
                        setup.index("chown root:root /opt"))
        self.assertLess(setup.index("REFUSE: existing TOR/fixture path"),
                        setup.index("chown root:root /opt"))
        self.assertLess(setup.index("chown root:root /opt"),
                        setup.index("DEPLOY08A_TRUSTED_FIXTURE_PARENTS_OK"))
        self.assertLess(setup.index("DEPLOY08A_TRUSTED_FIXTURE_PARENTS_OK"),
                        setup.index("DEPLOY08A_HOSTED_VM_FIXTURE_READY"))
        self.assertEqual([line for line in setup.splitlines() if line.startswith("chown ")],
                         ["chown root:root /opt"])
        self.assertIn("chmod 0755 /opt", setup)
        self.assertIn("(( (8#$mode & 022) == 0 ))", setup)
        self.assertIn("/etc/nginx/sites-available /etc/nginx/sites-enabled /etc/nginx/conf.d", setup)

    def test_legacy_sudoers_preflight_is_narrow_and_fixture_only(self):
        import ast
        lab = ROOT / "tests/deploy/hosted_vm_acceptance.py"
        text = lab.read_text()
        parser = next(node for node in ast.parse(text).body
                      if isinstance(node, ast.FunctionDef)
                      and node.name == "parsed_vm_sudoers_paths")
        module = ast.Module(body=[parser], type_ignores=[])
        namespace = {"Path": Path, "re": __import__("re")}
        exec(compile(module, str(lab), "exec"), namespace)
        paths = namespace["parsed_vm_sudoers_paths"](
            b"/etc/sudoers: parsed OK\n"
            b"/etc/sudoers.d/runner: parsed OK\n"
            b"/etc/sudoers.d/90-cloud-init-users: parsed OK\n"
            b"/tmp/sudoers: parsed OK\n"
            b"/etc/sudoers.d/../sudoers: parsed OK\n"
            b"/etc/sudoers.d/other.conf: parsed OK\n"
            b"/etc/sudoers.d/unsafe: syntax error\n")
        self.assertEqual(list(paths), [Path("/etc/sudoers"), Path("/etc/sudoers.d/runner"),
                                      Path("/etc/sudoers.d/90-cloud-init-users")])
        self.assertLess(text.index("checkout = hosted_guard()"),
                        text.index('require_vm_sudoers("existing", normalize_runner_modes=True)'))
        self.assertIn("stat.S_IMODE(info.st_mode) == 0o644", text)
        self.assertIn("os.fchmod(fd, 0o440)", text)
        self.assertIn('require_vm_sudoers("after_tor_rule")', text)
        production = (ROOT / "tools/install_release_v2.py").read_text()
        self.assertIn('executor.command("visudo", "-c")', production)
        self.assertNotIn("normalize_runner_modes", production)

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
