"""Keep proprietary workspace metadata separate from third-party licenses."""
from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]


def load(path):
    return tomllib.loads((ROOT / path).read_text(encoding="utf-8"))


class RustSecLicensePolicyTests(unittest.TestCase):
    def test_workspace_packages_inherit_nonpublication_and_ownership_notice(self):
        workspace = load("Cargo.toml")["workspace"]
        common = workspace["package"]
        self.assertIs(common["publish"], False)
        self.assertEqual(common["license-file"], "LICENSE")
        self.assertNotIn("license", common)
        self.assertTrue((ROOT / "LICENSE").is_file())
        self.assertIn("All rights reserved", (ROOT / "LICENSE").read_text())
        manifests = sorted(
            str(path.relative_to(ROOT))
            for path in (ROOT / "crates").glob("*/Cargo.toml")
        )
        self.assertEqual(
            manifests,
            sorted(f"{member}/Cargo.toml" for member in workspace["members"]),
        )
        for manifest in manifests:
            with self.subTest(manifest=manifest):
                package = load(manifest)["package"]
                self.assertEqual(package["publish"], {"workspace": True})
                self.assertEqual(package["license-file"], {"workspace": True})
                self.assertNotIn("license", package)

    def test_no_external_registry_license_bypass_or_advisory_exemption(self):
        policy = load("deny.toml")
        self.assertEqual(policy["advisories"]["ignore"], [])
        self.assertEqual(policy["advisories"]["yanked"], "deny")
        self.assertEqual(policy["advisories"]["unmaintained"], "all")
        self.assertEqual(policy["advisories"]["unsound"], "all")
        self.assertEqual(policy["bans"]["wildcards"], "deny")
        self.assertEqual(policy["sources"]["unknown-registry"], "deny")
        self.assertEqual(policy["sources"]["unknown-git"], "deny")
        licenses = policy["licenses"]
        self.assertIs(licenses["include-dev"], True)
        self.assertIs(licenses["include-build"], True)
        self.assertEqual(
            licenses["private"],
            {"ignore": True, "registries": [], "ignore-sources": []},
        )
        self.assertNotIn("CDLA-Permissive-2.0", licenses["allow"])
        self.assertEqual(licenses["exceptions"], [{
            "name": "webpki-roots",
            "version": "=1.0.12",
            "allow": ["CDLA-Permissive-2.0"],
        }])
        lock = load("Cargo.lock")
        roots = [p["version"] for p in lock["package"] if p["name"] == "webpki-roots"]
        self.assertEqual(roots, ["1.0.12"])
        license_text = ROOT / "LICENSES/CDLA-Permissive-2.0.txt"
        self.assertIn("2. Conditions for Sharing Data", license_text.read_text())


if __name__ == "__main__":
    unittest.main()
