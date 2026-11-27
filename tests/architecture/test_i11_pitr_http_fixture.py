"""Exercise the actual schema-7 runtime archive parser with synthetic bytes."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "pitr_http_fixture", ROOT / "tools/i11_pitr_http_fixture.py"
)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def valid_members():
    return [
        ("fixture.dump", b"PGDMP synthetic fixture", tarfile.REGTYPE),
        ("rights.json", b"{}", tarfile.REGTYPE),
        ("pack.zst", b"synthetic PACK", tarfile.REGTYPE),
        ("pack.manifest.json", b"{}", tarfile.REGTYPE),
    ]


class RuntimeArchiveParser(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name) / "work"
        self.work.mkdir()

    def write_archive(self, members):
        archive = self.work / "runtime.tar"
        with tarfile.open(archive, "w") as tar:
            for name, data, kind in members:
                member = tarfile.TarInfo(name)
                member.type = kind
                if kind == tarfile.REGTYPE:
                    member.size = len(data)
                    tar.addfile(member, io.BytesIO(data))
                else:
                    member.linkname = "fixture.dump"
                    tar.addfile(member)
        return archive

    def test_exact_allowlist_extracts_original_bytes(self):
        members = valid_members()
        self.write_archive(members)
        fixture.unpack(self.work)
        self.assertEqual(
            {p.name for p in self.work.iterdir()},
            {"runtime.tar", *fixture.LIMITS},
        )
        for name, data, _ in members:
            self.assertEqual((self.work / name).read_bytes(), data)

    def test_missing_member_is_rejected(self):
        self.write_archive(valid_members()[:-1])
        with self.assertRaisesRegex(ValueError, "incomplete runtime bundle"):
            fixture.unpack(self.work)

    def test_duplicate_member_is_rejected(self):
        members = valid_members()
        self.write_archive(members + [members[0]])
        with self.assertRaisesRegex(ValueError, "unexpected runtime bundle member"):
            fixture.unpack(self.work)
        self.assertEqual(
            (self.work / members[0][0]).read_bytes(), members[0][1]
        )

    def test_unknown_member_is_rejected(self):
        self.write_archive([("extra.txt", b"extra", tarfile.REGTYPE)])
        with self.assertRaisesRegex(ValueError, "unexpected runtime bundle member"):
            fixture.unpack(self.work)
        self.assertFalse((self.work / "extra.txt").exists())

    def test_paths_are_exact_names_not_normalized(self):
        for name in ("./rights.json", "subdir/rights.json", "../outside.txt"):
            with self.subTest(name=name):
                self.write_archive([(name, b"extra", tarfile.REGTYPE)])
                with self.assertRaisesRegex(ValueError, "unexpected runtime bundle member"):
                    fixture.unpack(self.work)
        self.assertFalse((self.work.parent / "outside.txt").exists())

    def test_links_and_nonregular_members_are_rejected(self):
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.DIRTYPE, tarfile.FIFOTYPE):
            with self.subTest(kind=kind):
                self.write_archive([("rights.json", b"", kind)])
                with self.assertRaisesRegex(ValueError, "unexpected runtime bundle member"):
                    fixture.unpack(self.work)
                self.assertFalse((self.work / "rights.json").exists())

    def test_zero_and_oversize_members_are_rejected(self):
        for data in (b"", b"x" * (fixture.LIMITS["rights.json"] + 1)):
            with self.subTest(size=len(data)):
                self.write_archive([("rights.json", data, tarfile.REGTYPE)])
                with self.assertRaisesRegex(ValueError, "unexpected runtime bundle member"):
                    fixture.unpack(self.work)

    def test_archive_size_is_bounded_before_parsing(self):
        archive = self.work / "runtime.tar"
        with archive.open("wb") as output:
            output.truncate(fixture.MAX_BUNDLE + 1)
        with self.assertRaisesRegex(ValueError, "invalid runtime bundle length"):
            fixture.unpack(self.work)

    def test_incorrect_postgresql_magic_is_rejected(self):
        members = valid_members()
        members[0] = ("fixture.dump", b"not a PGDMP archive", tarfile.REGTYPE)
        self.write_archive(members)
        with self.assertRaisesRegex(ValueError, "expected custom PostgreSQL archive"):
            fixture.unpack(self.work)

    def test_existing_destination_is_never_overwritten(self):
        self.write_archive(valid_members())
        target = self.work / "fixture.dump"
        target.write_bytes(b"existing bytes")
        with self.assertRaises(FileExistsError):
            fixture.unpack(self.work)
        self.assertEqual(target.read_bytes(), b"existing bytes")

    def test_truncated_archive_is_rejected(self):
        archive = self.write_archive(valid_members())
        with archive.open("r+b") as output:
            output.truncate(512)
        with self.assertRaises((tarfile.TarError, ValueError, OSError)):
            fixture.unpack(self.work)


if __name__ == "__main__":
    unittest.main()
