DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM users u
        JOIN account_settings s ON s.user_id=u.id
        JOIN books b ON b.owner_user_id=u.id
        WHERE u.email_normalized='privacy-legacy@example.invalid'
          AND u.display_name ~ '^Читатель №[0-9a-f]{32}$'
          AND u.password_hash='$argon2id$privacy-fixture'
          AND s.handle='privacy_legacy' AND s.revision=8
          AND b.source_identity='ci:privacy:retained-owner'
    ) THEN
        RAISE EXCEPTION 'Privacy backfill changed account identity, credentials, or handle';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM users u JOIN account_settings s ON s.user_id=u.id
        WHERE u.email_normalized='privacy-second@example.invalid'
          AND u.display_name ~ '^Читатель №[0-9a-f]{32}$'
          AND s.handle IS NULL AND s.revision=1
    ) THEN
        RAISE EXCEPTION 'Privacy backfill did not repair account without settings';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM users u
        WHERE u.email_normalized='privacy-safe@example.invalid'
          AND u.display_name='Safe Reader'
          AND NOT EXISTS (SELECT 1 FROM account_settings s WHERE s.user_id=u.id)
    ) THEN
        RAISE EXCEPTION 'Privacy backfill modified a valid account';
    END IF;
    IF EXISTS (
        SELECT 1 FROM users WHERE strpos(lower(display_name), email_normalized)>0
    ) THEN
        RAISE EXCEPTION 'Legacy public email remained in display name';
    END IF;
    BEGIN
        UPDATE users SET display_name='PRIVACY-LEGACY@EXAMPLE.INVALID'
        WHERE email_normalized='privacy-legacy@example.invalid';
        RAISE EXCEPTION 'Privacy constraint allowed a direct update';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;
END $$;
