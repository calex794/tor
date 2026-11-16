#!/usr/bin/env python3
"""TOR-STORE experimental public Zstd seekable pack and independently trusted manifest.

No user text is committed. No TOR rights or payment decisions live here.
Format reference: facebook/zstd/contrib/seekable_format/zstd_seekable_compression_format.md
This is a CI research prototype, not a production storage adapter.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import struct

import zstandard as zstd


SEEK_TABLE_SKIPPABLE_MAGIC = 0x184D2A5E
SEEKABLE_MAGIC = 0x8F92EAB1
MAX_FRAMES = 100_000
MAX_RAW_MEMBER = 8 * 2**20
MAX_MANIFEST = 64 * 2**20


class PackError(ValueError):
    """Fail-closed validation of a pack, its manifest, or an encoded chapter."""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical_json(obj: dict) -> bytes:
    return (json.dumps(obj, ensure_ascii=False, sort_keys=True,
                       separators=(",", ":")) + "\n").encode("utf-8")


def build_pack(editions: list[tuple[str, bytes]], level: int = 3) -> tuple[bytes, bytes]:
    """Create a standards-compatible seekable .zst and versioned sidecar manifest.

    The manifest must be stored with a digest in a trusted catalog or verified
    outside the pack. The seek table's optional XXH64 checksums are disabled:
    every edition has a separate cryptographic SHA-256 in the trusted manifest.
    """
    if not 1 <= len(editions) <= MAX_FRAMES:
        raise PackError("invalid edition count")
    if not 1 <= level <= 19:
        raise PackError("unexpected compression level")

    compressor = zstd.ZstdCompressor(level=level)
    ids, frames, table, manifest_entries = set(), [], bytearray(), []
    offset = 0
    for i, (edition_id, raw) in enumerate(editions):
        if (not isinstance(edition_id, str) or not edition_id
                or len(edition_id.encode("utf-8")) > 128 or "\x00" in edition_id
                or edition_id in ids):
            raise PackError("invalid/duplicate edition id")
        if not isinstance(raw, bytes) or not 0 < len(raw) <= MAX_RAW_MEMBER:
            raise PackError("invalid raw edition bytes")
        ids.add(edition_id)
        encoded = compressor.compress(raw)
        if not 0 < len(encoded) <= 2**32 - 1:
            raise PackError("encoded frame exceeds seekable format")
        frames.append(encoded)
        table.extend(struct.pack("<II", len(encoded), len(raw)))
        manifest_entries.append({
            "edition_id": edition_id, "frame": i, "offset": offset,
            "encoded_bytes": len(encoded), "raw_bytes": len(raw),
            "sha256": sha256(raw),
        })
        offset += len(encoded)

    # Seekable format 0.1.0: skippable magic + frame-size (8 * N + 9)
    # + (compressed_size, decompressed_size) per frame
    # + 9-byte footer (frame_count, descriptor=0, seekable_magic).
    table_payload_length = len(table) + 9
    seek_table = (struct.pack("<II", SEEK_TABLE_SKIPPABLE_MAGIC, table_payload_length)
                  + bytes(table)
                  + struct.pack("<IBI", len(editions), 0, SEEKABLE_MAGIC))
    pack = b"".join(frames) + seek_table
    manifest = {
        "format": "tor-store-public-seekable-zstd",
        "manifest_version": 1,
        "zstd_seekable_version": "0.1.0",
        "pack_sha256": sha256(pack),
        "pack_encoded_bytes": len(pack),
        "seek_table_offset": offset,
        "seek_table_bytes": len(seek_table),
        "frames": len(editions),
        "entries": manifest_entries,
    }
    result = canonical_json(manifest)
    if len(result) > MAX_MANIFEST:
        raise PackError("manifest exceeds bound")
    return pack, result


def parse_seek_table(table: bytes, total_pack_bytes: int) -> list[tuple[int, int, int]]:
    """Parse the entire seek table from a known verified tail range."""
    if len(table) < 25:
        raise PackError("truncated seek table")
    magic, size = struct.unpack_from("<II", table)
    if magic != SEEK_TABLE_SKIPPABLE_MAGIC or size != len(table) - 8:
        raise PackError("invalid seek table wrapper")
    frames, flags, footer = struct.unpack_from("<IBI", table, len(table) - 9)
    if not 1 <= frames <= MAX_FRAMES or flags != 0 or footer != SEEKABLE_MAGIC:
        raise PackError("invalid seek table footer")
    if len(table) != 8 + frames * 8 + 9:
        raise PackError("mismatched seek table entry count")
    offset, entries = 0, []
    for i in range(frames):
        compressed_size, raw_size = struct.unpack_from("<II", table, 8 + i * 8)
        if compressed_size == 0 or not 0 < raw_size <= MAX_RAW_MEMBER:
            raise PackError("out-of-bounds frame entry")
        entries.append((offset, compressed_size, raw_size))
        offset += compressed_size
    if offset != total_pack_bytes - len(table):
        raise PackError("compressed offsets disagree with seek table start")
    return entries


def parse_manifest(manifest_bytes: bytes, expected_digest: str) -> dict:
    """The digest must come from a trusted DB/backup, not the untrusted S3 object."""
    if not 0 < len(manifest_bytes) <= MAX_MANIFEST:
        raise PackError("manifest size")
    if sha256(manifest_bytes) != expected_digest:
        raise PackError("manifest hash mismatch")
    try:
        manifest = json.loads(manifest_bytes)
    except (ValueError, UnicodeDecodeError) as exc:
        raise PackError("malformed manifest") from exc
    if (not isinstance(manifest, dict)
            or manifest.get("format") != "tor-store-public-seekable-zstd"
            or manifest.get("manifest_version") != 1
            or manifest.get("zstd_seekable_version") != "0.1.0"):
        raise PackError("unknown manifest format")
    count = manifest.get("frames")
    entries = manifest.get("entries")
    size = manifest.get("pack_encoded_bytes")
    tail = manifest.get("seek_table_bytes")
    start = manifest.get("seek_table_offset")
    if (not isinstance(count, int) or not 1 <= count <= MAX_FRAMES
            or not isinstance(entries, list) or len(entries) != count
            or not isinstance(size, int) or not isinstance(start, int)
            or not isinstance(tail, int) or start + tail != size
            or tail != count * 8 + 17):
        raise PackError("manifest structural mismatch")
    ids, offset = set(), 0
    for i, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise PackError("invalid member")
        eid = entry.get("edition_id")
        enc, raw = entry.get("encoded_bytes"), entry.get("raw_bytes")
        digest = entry.get("sha256")
        if (not isinstance(eid, str) or not eid or eid in ids
                or not isinstance(enc, int) or enc <= 0
                or not isinstance(raw, int) or not 0 < raw <= MAX_RAW_MEMBER
                or not isinstance(digest, str) or len(digest) != 64
                or any(c not in "0123456789abcdef" for c in digest)
                or entry.get("frame") != i or entry.get("offset") != offset):
            raise PackError("invalid member offset/identity/digest")
        ids.add(eid)
        offset += enc
    if offset != start:
        raise PackError("manifest compressed size mismatch")
    return manifest


def extract_range(reader, pack_size: int, manifest_bytes: bytes,
                  trusted_manifest_sha256: str, edition_id: str) -> bytes:
    """Read exact byte ranges of one edition, validating the persisted index.

    reader(offset, size) must fetch exactly that many bytes (S3 Range GET).
    This makes 2 ranges per extraction in a cold prototype. In TOR, trusted
    validated seek-table entries would be cached by immutable pack SHA.
    """
    manifest = parse_manifest(manifest_bytes, trusted_manifest_sha256)
    if pack_size != manifest["pack_encoded_bytes"]:
        raise PackError("remote object changed length")
    by_id = {e["edition_id"]: e for e in manifest["entries"]}
    entry = by_id.get(edition_id)
    if entry is None:
        raise PackError("requested edition absent from verified manifest")

    tail_size = manifest["seek_table_bytes"]
    tail = reader(pack_size - tail_size, tail_size)
    if len(tail) != tail_size:
        raise PackError("short read of seek table")
    table = parse_seek_table(tail, pack_size)
    offset, compressed_size, raw_size = table[entry["frame"]]
    if (offset != entry["offset"]
            or compressed_size != entry["encoded_bytes"]
            or raw_size != entry["raw_bytes"]):
        raise PackError("manifest disagrees with seek table")

    encoded = reader(offset, compressed_size)
    if len(encoded) != compressed_size:
        raise PackError("short read of compressed edition")
    try:
        raw = zstd.ZstdDecompressor().decompress(
            encoded, max_output_size=raw_size)
    except zstd.ZstdError as exc:
        raise PackError("compressed frame checksum/stream failure") from exc
    if len(raw) != raw_size or sha256(raw) != entry["sha256"]:
        raise PackError("edition content hash/length mismatch")
    return raw


def self_test():
    """Independent content, corrupted frame/table, forged manifest and bounds."""
    originals = [
        ("v1-immutable", ("Первая глава.\n" * 50).encode("utf-8")),
        ("v2-immutable", ("Вторая глава — новая редакция.\n" * 70).encode("utf-8")),
        ("v3-immutable", ("独立的章节内容\n" * 80).encode("utf-8")),
    ]
    pack, mbytes = build_pack(originals)
    trusted = sha256(mbytes)
    metadata = parse_manifest(mbytes, trusted)
    assert metadata["frames"] == len(originals)
    assert metadata["pack_sha256"] == sha256(pack)
    assert metadata["seek_table_offset"] + metadata["seek_table_bytes"] == len(pack)

    def reader(source):
        return lambda offset, length: source[offset:offset + length]

    for edition_id, raw in originals:
        assert extract_range(reader(pack), len(pack), mbytes, trusted, edition_id) == raw

    def must_reject(cb):
        try:
            cb()
        except PackError:
            return
        raise AssertionError("corruption was accepted")

    must_reject(lambda: build_pack([originals[0], originals[0]]))
    must_reject(lambda: extract_range(reader(pack), len(pack), mbytes, trusted, "missing"))
    must_reject(lambda: extract_range(reader(pack), len(pack) + 1, mbytes, trusted, "v1-immutable"))
    must_reject(lambda: extract_range(lambda o, n: b"", len(pack),
                                     mbytes, trusted, "v1-immutable"))

    damaged = bytearray(pack)
    damaged[metadata["entries"][1]["offset"] + 6] ^= 0x4F
    must_reject(lambda: extract_range(reader(damaged), len(damaged),
                                     mbytes, trusted, "v2-immutable"))

    damaged_tail = bytearray(pack)
    damaged_tail[-1] ^= 0xFF
    must_reject(lambda: extract_range(reader(damaged_tail), len(damaged_tail),
                                     mbytes, trusted, "v1-immutable"))

    forged = json.loads(mbytes)
    forged["entries"][0]["sha256"] = "a" * 64
    modified = canonical_json(forged)
    must_reject(lambda: extract_range(reader(pack), len(pack), modified,
                                     trusted, "v1-immutable"))
    assert len(pack) > metadata["seek_table_offset"]
    print("TOR-STORE Seekable v0.1.0 + trusted manifest + tamper preflight: PASS")
    print(f"seekable_payload_bytes={metadata['seek_table_offset']} "
          f"seek_table_bytes={metadata['seek_table_bytes']} "
          f"manifest_bytes={len(mbytes)}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
    else:
        parser.error("this tool is an experimental library; use --self-test")


if __name__ == "__main__":
    main()
