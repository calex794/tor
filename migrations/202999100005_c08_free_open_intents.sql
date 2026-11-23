-- C-08: a signed/free PACK source was authorized before the browser
-- downloaded and SHA-checked its exact bytes. Author V2 publication in the
-- meantime must not silently discard a real V1 open acknowledgement.
-- No permanent pin, reading right or S3 DELETE eligibility is created here.
CREATE TABLE reader_free_open_intents (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256)=32),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id,chapter_id,version_id),
    CONSTRAINT c08_free_open_intent_exact_version_fk
      FOREIGN KEY(chapter_id,version_id)
      REFERENCES chapter_versions(chapter_id,id) ON DELETE CASCADE
);
CREATE INDEX c08_free_open_intents_expiry_idx
    ON reader_free_open_intents (expires_at);
COMMENT ON TABLE reader_free_open_intents IS
    'Short-lived proof of previously authorized exact FREE delivery, not proof of human reading, a paid right, or S3 GC authorization. Expired intents require separate bounded PG cleanup.';
