-- Package images are independent from book covers and from paid content.
-- Only a moderated, approved candidate can be publicly served.
CREATE TABLE commerce_layer_cover_candidates (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    layer_id BIGINT NOT NULL REFERENCES commerce_layers(id) ON DELETE RESTRICT,
    uploaded_by_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'uploading'
        CHECK (status IN ('uploading','pending','approved','rejected','superseded','failed')),
    card_key TEXT NOT NULL CHECK (
        card_key LIKE 'staging/covers/variants/packages/%'
        AND length(card_key) <= 512),
    main_key TEXT NOT NULL CHECK (
        main_key LIKE 'staging/covers/variants/packages/%'
        AND length(main_key) <= 512
        AND main_key <> card_key),
    card_sha256 BYTEA NOT NULL CHECK (octet_length(card_sha256) = 32),
    main_sha256 BYTEA NOT NULL CHECK (octet_length(main_sha256) = 32),
    card_bytes BIGINT NOT NULL CHECK (card_bytes BETWEEN 1 AND 8388608),
    main_bytes BIGINT NOT NULL CHECK (main_bytes BETWEEN 1 AND 8388608),
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reviewed_at TIMESTAMPTZ,
    terminal_at TIMESTAMPTZ,
    reviewed_by_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    review_reason TEXT NOT NULL DEFAULT '' CHECK (char_length(review_reason) <= 2000),
    CHECK (
        (status IN ('pending','uploading','failed') AND reviewed_at IS NULL AND reviewed_by_user_id IS NULL)
        OR (status IN ('approved','rejected','superseded') AND reviewed_at IS NOT NULL AND reviewed_by_user_id IS NOT NULL)
    ),
    CHECK ((status IN ('failed','rejected','superseded')) = (terminal_at IS NOT NULL))
);
CREATE UNIQUE INDEX commerce_package_cover_inflight_unique
    ON commerce_layer_cover_candidates(layer_id)
    WHERE status IN ('uploading','pending');
CREATE UNIQUE INDEX commerce_package_cover_approved_unique
    ON commerce_layer_cover_candidates(layer_id)
    WHERE status='approved';
CREATE INDEX commerce_package_cover_queue
    ON commerce_layer_cover_candidates(status,submitted_at,id)
    WHERE status='pending';
CREATE INDEX commerce_package_cover_owner_history
    ON commerce_layer_cover_candidates(layer_id,id DESC);

-- Deferred, fenced private cleanup: never delete a live approval or review history.
CREATE TABLE commerce_layer_cover_variant_removals (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    candidate_id BIGINT NOT NULL REFERENCES commerce_layer_cover_candidates(id) ON DELETE RESTRICT,
    variant_name TEXT NOT NULL CHECK (variant_name IN ('main','card')),
    storage_key TEXT NOT NULL UNIQUE CHECK (
        storage_key LIKE 'staging/covers/variants/packages/%'
        AND char_length(storage_key) <= 512),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','leased','removed')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    locked_until TIMESTAMPTZ,
    retry_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    removed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (candidate_id,variant_name),
    CHECK ((state='removed') = (removed_at IS NOT NULL))
);
CREATE INDEX commerce_package_cover_cleanup_claim
    ON commerce_layer_cover_variant_removals(retry_after,id)
    WHERE state IN ('pending','leased');
