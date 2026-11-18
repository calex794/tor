-- FND-00 / #1103: disposable PostgreSQL proof of the paid bodyless S3 contract.
-- The ciphertext is deliberately not uploaded: this asserts only SQL receipt,
-- publication, version identity and negative guard behavior. End-to-end MinIO
-- and real private reader acceptance belong to the subsequent Rust slice.

DO $fnd00_bodyless_sql_contract$
<<fixture>>
DECLARE
    actor BIGINT;
    book_id BIGINT;
    chapter_id BIGINT;
    version_v1 BIGINT;
    version_v2 BIGINT;
    version_legacy BIGINT;
    object_v1 UUID := 'c2a6f138-93ed-4a01-b7c7-41ea87911801'::uuid;
    bad_object UUID := 'c2a6f138-93ed-4a01-b7c7-41ea87911802'::uuid;
    source_v1 TEXT := '# FND-00 paid bodyless V1';
    source_v2 TEXT := '# FND-00 paid bodyless V2';
    source_legacy TEXT := '# FND-00 legacy PG paid';
    digest_v1 BYTEA;
BEGIN
    digest_v1 := sha256(convert_to(source_v1, 'UTF8'));
    INSERT INTO users(email_normalized, display_name, password_hash)
    VALUES ('fnd00-bodyless@example.invalid', 'FND00 Paid', '$argon2id$ci-only')
    RETURNING id INTO actor;

    INSERT INTO books(source_identity, target_language, title, visibility, owner_user_id)
    VALUES ('ci-fnd00-paid-s3', 'ru', 'Bodyless paid fixture', 'public', actor)
    RETURNING id INTO book_id;

    INSERT INTO chapters(book_id, chapter_no, title,
                         visibility, access_level, price_amount)
    VALUES (book_id, 1, 'Paid chapter', 'public', 'paid', 150)
    RETURNING id INTO chapter_id;

    INSERT INTO chapter_versions(chapter_id, version_no, body_markdown,
                                 content_sha256)
    VALUES (chapter_id, 1, NULL, digest_v1)
    RETURNING id INTO version_v1;

    -- A bodyless paid version must not be published just because it has an ID.
    BEGIN
        INSERT INTO chapter_publications(chapter_id, version_id)
        VALUES (chapter_id, version_v1);
        RAISE EXCEPTION 'FND00 accepted publication before attached S3 receipt'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    -- G's prepared/verified receipt is separate from a buyer entitlement.
    INSERT INTO storage_objects(
        id, object_kind, bucket, object_key, state, size_bytes,
        sha256_ciphertext, plaintext_sha256, encrypted_dek, wrap_nonce,
        kek_id, object_nonce, object_aad, codec, verified_at
    )
    VALUES (
        object_v1, 'chapter', 'tor-fnd00-private',
        'private/chapter-versions/' || object_v1::text || '.enc',
        'ready', 256,
        decode(repeat('a1', 32), 'hex'), digest_v1,
        decode(repeat('b2', 48), 'hex'),
        decode(repeat('c3', 12), 'hex'),
        'content-kek-v1',
        decode(repeat('d4', 12), 'hex'),
        convert_to(
            'tor.encrypted.chapter.v1' || E'\n' ||
            chapter_id::text || E'\n' ||
            version_v1::text || E'\n' ||
            object_v1::text || E'\n' ||
            encode(digest_v1, 'hex') || E'\n',
            'UTF8'
        ),
        'gzip', clock_timestamp()
    );

    -- A ready but unattached object cannot pass the B association trigger.
    BEGIN
        UPDATE chapter_versions SET storage_object_id=object_v1
        WHERE id=version_v1;
        RAISE EXCEPTION 'FND00 accepted unattached G receipt'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    UPDATE storage_objects
       SET state='attached', attached_at=clock_timestamp()
     WHERE id=object_v1;
    UPDATE chapter_versions
       SET storage_object_id=object_v1
     WHERE id=version_v1;

    -- Even attached ciphertext is not publishable without the matching
    -- B-owned immutable reader coordinates.
    BEGIN
        INSERT INTO chapter_publications(chapter_id, version_id)
        VALUES (chapter_id, version_v1);
        RAISE EXCEPTION 'FND00 accepted bodyless paid publication without manifest'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    BEGIN
        INSERT INTO chapter_reader_manifests(
            chapter_id, version_id, storage_object_id, parser_schema,
            content_sha256, block_utf16_lengths,
            last_readable_block_index, last_readable_char_offset
        ) VALUES (
            chapter_id, version_v1, object_v1, 1,
            decode(repeat('fe',32),'hex'), ARRAY[char_length(source_v1)],
            1, char_length(source_v1)
        );
        RAISE EXCEPTION 'FND00 accepted a mismatched paid manifest digest'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    INSERT INTO chapter_reader_manifests(
        chapter_id, version_id, storage_object_id, parser_schema,
        content_sha256, block_utf16_lengths,
        last_readable_block_index, last_readable_char_offset
    ) VALUES (
        chapter_id, version_v1, object_v1, 1,
        digest_v1, ARRAY[char_length(source_v1)],
        1, char_length(source_v1)
    );

    INSERT INTO chapter_publications(chapter_id, version_id)
    VALUES (chapter_id, version_v1);
    INSERT INTO chapter_encrypted_release_events(
        chapter_id, version_id, storage_object_id
    ) VALUES (chapter_id, version_v1, object_v1);

    IF (SELECT body_markdown FROM chapter_versions WHERE id=version_v1)
       IS NOT NULL
       OR (SELECT storage_object_id FROM chapter_versions WHERE id=version_v1)
          IS DISTINCT FROM object_v1
       OR (SELECT version_id FROM chapter_publications cp WHERE cp.chapter_id=fixture.chapter_id)
          IS DISTINCT FROM version_v1 THEN
        RAISE EXCEPTION 'FND00 bodyless paid V1 was not published exactly';
    END IF;

    -- A later edition without attached ciphertext cannot displace V1.
    INSERT INTO chapter_versions(chapter_id, version_no, body_markdown,
                                 content_sha256)
    VALUES (chapter_id, 2, NULL, sha256(convert_to(source_v2, 'UTF8')))
    RETURNING id INTO version_v2;
    BEGIN
        UPDATE chapter_publications SET version_id=version_v2
        WHERE chapter_publications.chapter_id=fixture.chapter_id;
        RAISE EXCEPTION 'FND00 accepted unverified paid V2'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    -- A different G object with incorrect version-bound AAD must not attach.
    INSERT INTO storage_objects(
        id, object_kind, bucket, object_key, state, size_bytes,
        sha256_ciphertext, plaintext_sha256, encrypted_dek, wrap_nonce,
        kek_id, object_nonce, object_aad, codec,
        verified_at, attached_at
    )
    VALUES (
        bad_object, 'chapter', 'tor-fnd00-private',
        'private/chapter-versions/' || bad_object::text || '.enc',
        'attached', 256,
        decode(repeat('a2',32),'hex'),
        sha256(convert_to(source_v2,'UTF8')),
        decode(repeat('b2',48),'hex'),
        decode(repeat('c3',12),'hex'),
        'content-kek-v1',
        decode(repeat('d4',12),'hex'),
        convert_to('incorrect-version-bound-AAD-for-negative-test','UTF8'),
        'gzip', clock_timestamp(), clock_timestamp()
    );
    BEGIN
        UPDATE chapter_versions SET storage_object_id=bad_object
        WHERE id=version_v2;
        RAISE EXCEPTION 'FND00 attached paid V2 to object with wrong AAD'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    BEGIN
        UPDATE chapter_versions
        SET content_sha256=decode(repeat('aa',32),'hex')
        WHERE id=version_v1;
        RAISE EXCEPTION 'FND00 rewrote immutable paid V1 digest'
            USING ERRCODE='P0001';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    -- Legacy paid editions with PG bodies remain supported by the additive
    -- migration: do not strand existing purchases by requiring new S3.
    INSERT INTO chapter_versions(chapter_id, version_no,
                                 body_markdown, content_sha256)
    VALUES (chapter_id, 3, source_legacy,
            sha256(convert_to(source_legacy,'UTF8')))
    RETURNING id INTO version_legacy;
    UPDATE chapter_publications
       SET version_id=version_legacy
     WHERE chapter_publications.chapter_id=fixture.chapter_id;
    IF (SELECT version_id FROM chapter_publications
        WHERE chapter_publications.chapter_id=fixture.chapter_id) <> version_legacy THEN
        RAISE EXCEPTION 'FND00 broke legacy PG paid publication';
    END IF;
END
$fnd00_bodyless_sql_contract$;

SELECT 'FND00_PAID_BODYLESS_SQL_OK' AS result;
