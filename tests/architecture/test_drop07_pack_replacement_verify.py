"""Offline coverage of the staged replacement PACK evidence command."""

import copy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import drop07_pack_replacement_verify as cli


class PackReplacementEvidenceTest(unittest.TestCase):
    def setUp(self):
        staging = tempfile.TemporaryDirectory()
        self.addCleanup(staging.cleanup)
        self.root = Path(staging.name).resolve()
        self.plan = {"source_pack_id": 7, "packs": [], "replacements": []}
        for pack_id in (8, 9):
            stem = f"private/free-packs/ci/replacement-{pack_id}"
            body, sidecar = bytes([pack_id]) * 64, b"fixture manifest"
            pack = {
                "pack_id": pack_id, "bucket": "tor-ci-packs",
                "pack_key": stem + ".zst", "manifest_key": stem + ".manifest.json",
                "pack_bytes": len(body), "manifest_bytes": len(sidecar),
                "pack_sha256": hashlib.sha256(body).hexdigest(),
                "manifest_sha256": hashlib.sha256(sidecar).hexdigest(),
                "seek_table_start": 39,
            }
            for key, raw in ((pack["pack_key"], body), (pack["manifest_key"], sidecar)):
                path = self.root / key
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
            self.plan["packs"].append(pack)
            self.plan["replacements"].append({
                "chapter_id": pack_id, "version_id": 5, "source_generation": 1,
                "active_generation": 2, "pack_id": pack_id, "frame_offset": 0,
                "frame_bytes": 39, "size_bytes": 4, "content_sha256": "ab" * 32,
            })

    def test_exact_pairs_match_multiple_active_packs(self):
        self.assertEqual(cli.load_plan(json.dumps(self.plan).encode()), self.plan)
        cli.verify(self.plan, self.root)

    def test_same_size_change_in_either_object_is_rejected(self):
        for key in ("pack_key", "manifest_key"):
            with self.subTest(key=key):
                path = self.root / self.plan["packs"][0][key]
                raw = path.read_bytes()
                path.write_bytes(bytes([raw[0] ^ 1]) + raw[1:])
                with self.assertRaisesRegex(cli.Refused, "bytes differ"):
                    cli.verify(self.plan, self.root)
                path.write_bytes(raw)

    def test_a_distinct_but_unproven_active_pack_is_rejected(self):
        plan = copy.deepcopy(self.plan)
        plan["replacements"][0]["pack_id"] = 10
        with self.assertRaisesRegex(cli.Refused, "lacks verified"):
            cli.verify(plan, self.root)

    def test_refuses_an_out_of_bounds_frame(self):
        self.plan["replacements"][0]["frame_offset"] = 1
        with self.assertRaisesRegex(cli.Refused, "numeric field"):
            cli.verify(self.plan, self.root)

    def test_refuses_a_symlink_even_when_the_referent_has_the_expected_bytes(self):
        path = self.root / self.plan["packs"][0]["manifest_key"]
        other = self.root / "other-manifest.json"
        path.rename(other)
        path.symlink_to(other)
        with self.assertRaisesRegex(cli.Refused, "symlinked"):
            cli.verify(self.plan, self.root)

    def test_empty_and_duplicate_json_fields_are_rejected(self):
        for raw in (b"", b'{"source_pack_id":7,"source_pack_id":8}'):
            with self.subTest(raw=raw), self.assertRaises(cli.Refused):
                cli.load_plan(raw)
