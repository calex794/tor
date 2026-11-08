"""Offline S025 staging verifier tests, without any external network."""
import hashlib
import unittest
from unittest.mock import patch

from s025_cdn_staging_probe import (
    ProbeFailure, Result, browser_max_age, evaluate, main, validate_target,
)


class S025StagingProbeTests(unittest.TestCase):
    def setUp(self):
        self.data = b"RIFF\x00\x00\x00\x00WEBPtest-only"
        self.digest = hashlib.sha256(self.data).hexdigest()
        self.host = "cdn-staging.example.net"
        self.url = f"https://{self.host}/public/covers/950/7/card-{self.digest}.webp"

    def test_exact_staging_url(self):
        self.assertEqual(validate_target(self.url, self.host), self.digest)
        for url, hostname in [
            (self.url.replace("https:", "http:"), self.host),
            (self.url, "cdn-prod.example.net"),
            (self.url.replace("cdn-staging", "cdn-prod"), "cdn-prod.example.net"),
            (self.url + "?query=1", self.host),
            (self.url.replace(".webp", ".webp*"), self.host),
            (self.url.replace("/950/7/", "/950/../"), self.host),
            (self.url.replace("https://", "https://user:pass@"), self.host),
            (self.url.replace("example.net", "example.net:8443"), self.host),
        ]:
            with self.subTest(url=url), self.assertRaises(ProbeFailure):
                validate_target(url, hostname)

    def test_bounded_cache_ttl(self):
        for header, max_age in [
            ("public, max-age=60, must-revalidate", 60),
            ("public, max-age=0", 0), ("no-store", 0), ("no-cache", 0),
        ]:
            self.assertEqual(browser_max_age(header), max_age)
        for header in ["", "public", "max-age=61", "max-age=31536000, immutable",
                       "max-age=60, max-age=61", "max-age=no"]:
            with self.subTest(header=header), self.assertRaises(ProbeFailure):
                browser_max_age(header)

    def test_before_checks_mime_digest_and_ttl(self):
        good = Result(200, "image/webp", "public, max-age=60", self.digest)
        self.assertEqual(
            evaluate("before", self.digest, lambda: good)["sha256"], "verified"
        )
        for bad in [
            Result(404, "", "", None),
            Result(200, "text/html", "max-age=60", self.digest),
            Result(200, "image/webp", "max-age=60", "0" * 64),
            Result(200, "image/webp", "max-age=31536000", self.digest),
        ]:
            with self.subTest(status=bad.status), self.assertRaises(ProbeFailure):
                evaluate("before", self.digest, lambda: bad)

    def test_after_requires_three_negative_gets(self):
        replies = iter([403, 404, 410])
        result = evaluate("after", self.digest,
                          lambda: Result(next(replies), "", "", None),
                          pause=lambda _: None)
        self.assertEqual(result["statuses"], [403, 404, 410])
        self.assertFalse(result["old_bytes_served"])
        for code in [200, 206, 302, 500]:
            with self.subTest(code=code), self.assertRaises(ProbeFailure):
                evaluate("after", self.digest,
                         lambda: Result(code, "", "", None),
                         pause=lambda _: None)

    def test_preflight_never_calls_network(self):
        with patch("s025_cdn_staging_probe.read_only_get",
                   side_effect=AssertionError("unexpected network")):
            exit_code = main(["--phase", "before", "--url", self.url,
                              "--staging-host", self.host])
        self.assertEqual(exit_code, 0)


if __name__ == "__main__":
    unittest.main()
