-- G-07/G-09 + B/S025. Additive migration; the nine donor migrations remain immutable.
-- A verified source is registered only by the trusted upload/finalize path after
-- bounded S3 HEAD/GET/hash/MIME checks. There is no public endpoint for marking it verified.
CREATE TABLE cover_upload_objects (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    uploader_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    storage_key TEXT NOT NULL UNIQUE
        CHECK (char_length(storage_key) BETWEEN 16 AND 512
               AND storage_key LIKE 'staging/covers/%'),
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256) = 32),
    content_type TEXT NOT NULL CHECK (content_type IN
        ('image/jpeg','image/png','image/webp')),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 1 AND 8388608),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','verified','quarantined')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX cover_upload_by_book ON cover_upload_objects(book_id, id DESC);

-- Durable single-source idempotency. A job is a proposed cover generation;
-- it does not modify a Book's current cover until the B attachment CAS.
CREATE TABLE cover_variant_jobs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_object_id BIGINT NOT NULL REFERENCES cover_upload_objects(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    actor_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    recipe_version TEXT NOT NULL CHECK (char_length(recipe_version) BETWEEN 8 AND 100),
    expected_slot_revision BIGINT NOT NULL CHECK (expected_slot_revision >= 0),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','running','ready','done','superseded','dead')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    locked_until TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 10),
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error_code TEXT CHECK (char_length(last_error_code) <= 100),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source_object_id, recipe_version)
);
CREATE INDEX cover_variant_job_claim_idx
    ON cover_variant_jobs(run_after, id)
    WHERE status IN ('pending','running','ready');

-- Two immutable WebP outputs, only visible to B when both exist and S3 HEAD
-- verification has completed. Orphans from a failed S3 PUT can later be GC'd.
CREATE TABLE cover_variants (
    job_id BIGINT NOT NULL REFERENCES cover_variant_jobs(id) ON DELETE RESTRICT,
    variant_name TEXT NOT NULL CHECK (variant_name IN ('main','card')),
    storage_key TEXT NOT NULL UNIQUE CHECK (
        storage_key LIKE 'staging/covers/variants/%'
        AND char_length(storage_key) BETWEEN 28 AND 512
    ),
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256)=32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 1 AND 8388608),
    verified_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (job_id, variant_name)
);

-- The B-owned book cover pointer is switched atomically only by B repository.
-- No G job or S3 retry is allowed to write this table directly.
CREATE TABLE book_cover_slots (
    book_id BIGINT PRIMARY KEY REFERENCES books(id) ON DELETE RESTRICT,
    job_id BIGINT NOT NULL REFERENCES cover_variant_jobs(id) ON DELETE RESTRICT,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    attached_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Even if a future code path forgets the owner check, no one can attach a
-- different book's job to a book_cover_slots row.
CREATE FUNCTION reject_cross_book_cover_slot()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM cover_variant_jobs j
        WHERE j.id=NEW.job_id AND j.book_id=NEW.book_id
    ) THEN
        RAISE EXCEPTION 'cover slot references another book'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER cover_slots_same_book
    BEFORE INSERT OR UPDATE ON book_cover_slots
    FOR EACH ROW EXECUTE FUNCTION reject_cross_book_cover_slot();
