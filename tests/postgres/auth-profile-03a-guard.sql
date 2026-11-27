DO $$
BEGIN
    BEGIN
        INSERT INTO users(email_normalized,display_name,password_hash)
        VALUES ('privacy-fresh@example.invalid',
                'PRIVACY-FRESH@EXAMPLE.INVALID','$argon2id$privacy-fixture');
        RAISE EXCEPTION 'Fresh database allowed own email as public name';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;
    INSERT INTO users(email_normalized,display_name,password_hash)
    VALUES ('privacy-fresh@example.invalid','Fresh Reader','$argon2id$privacy-fixture');
END $$;
