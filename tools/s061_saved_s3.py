"""S061 fixture for one historical FREE edition stored only in loopback S3."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess

ENDPOINT = "http://127.0.0.1:9012"
BUCKET = "tor-s061-catalog-ci"
MARKDOWN = "S061 historical free edition V1"
AWS_ENV = {
    "AWS_ACCESS_KEY_ID": "tor-s061-ci-user",
    "AWS_SECRET_ACCESS_KEY": "tor-s061-ci-secret-only",
    "AWS_REGION": "us-east-1",
    "AWS_EC2_METADATA_DISABLED": "true",
    "AWS_ENDPOINT_URL_S3": ENDPOINT,
    "TOR_FREE_S3_BUCKET": BUCKET,
    "TOR_FREE_S3_PUBLIC_ORIGIN": ENDPOINT,
    "TOR_FREE_PACK_READER_ENABLED": "0",
}


class SavedChapterS3:
    def __init__(self, chapter: int, saved: int, current: int):
        if os.environ.get("TOR_S061_S3_ENDPOINT") != ENDPOINT:
            raise RuntimeError("S061 refuses a non-disposable S3 endpoint")
        self.chapter = chapter
        self.saved = saved
        self.current = current
        self.token = secrets.token_urlsafe(32)
        self.key = f"private/free-chapters/{chapter}/{saved}.md"
        self.sha256 = hashlib.sha256(MARKDOWN.encode()).hexdigest()

    def seed(self, sql, database: str) -> None:
        session_digest = hashlib.sha256(self.token.encode()).hexdigest()
        inserted = sql(f"""
            INSERT INTO sessions(token_hash, user_id, expires_at)
            SELECT decode('{session_digest}','hex'), id, now()+interval '2 hours'
            FROM users WHERE email_normalized='s061-catalog@example.invalid'
            RETURNING user_id;
        """, database)
        if not inserted.isdigit():
            raise RuntimeError("S061 saved reader session was not created")
        self.put(f"/{BUCKET}")
        self.put(f"/{BUCKET}/{self.key}", "--data-binary", "@-",
                 "--header", "If-None-Match: *",
                 "--header", "Content-Type: text/plain; charset=utf-8",
                 "--header", f"x-amz-meta-sha256: {self.sha256}", body=MARKDOWN.encode())

    @staticmethod
    def put(path: str, *args: str, body: bytes | None = None) -> None:
        key, secret = AWS_ENV["AWS_ACCESS_KEY_ID"], AWS_ENV["AWS_SECRET_ACCESS_KEY"]
        result = subprocess.run(
            ["curl", "--noproxy", "*", "--silent", "--show-error", "--fail", "--max-time", "15",
             "--aws-sigv4", "aws:amz:us-east-1:s3", "--user", f"{key}:{secret}",
             "--output", "/dev/null", "--request", "PUT", *args, ENDPOINT + path],
            input=body, timeout=20, capture_output=True,
        )
        if result.returncode:
            raise RuntimeError(f"S061 private MinIO fixture PUT failed (curl {result.returncode})")

    def assert_browser(self, port: int, stage: str) -> None:
        values = {
            "origin": f"http://127.0.0.1:{port}", "endpoint": ENDPOINT,
            "objectPath": f"/{BUCKET}/{self.key}", "chapter": str(self.chapter),
            "saved": str(self.saved), "current": str(self.current), "token": self.token,
            "markdown": MARKDOWN, "sha256": self.sha256,
        }
        script = Path(__file__).with_name("s061_saved_s3_chromium.cjs")
        subprocess.run(["node", str(script)], input=json.dumps(values), text=True,
                       timeout=45, check=True)
        print(f"S061_SAVED_FREE_S3_OK {stage}", flush=True)
