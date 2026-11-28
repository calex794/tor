-- Only the isolated installed-upgrade database, before the privacy migration.
WITH inserted AS (
    INSERT INTO users(email_normalized,display_name,password_hash)
    VALUES ('privacy-legacy@example.invalid',
            'Reader PRIVACY-LEGACY@EXAMPLE.INVALID',
            '$argon2id$privacy-fixture')
    RETURNING id
)
INSERT INTO account_settings(user_id,handle,revision)
SELECT id,'privacy_legacy',7 FROM inserted;

INSERT INTO users(email_normalized,display_name,password_hash)
VALUES ('privacy-safe@example.invalid','Safe Reader','$argon2id$privacy-safe'),
       ('privacy-second@example.invalid','privacy-second@example.invalid',
        '$argon2id$privacy-second');

INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
SELECT 'ci:privacy:retained-owner','ru','Privacy retained owner','draft',id
FROM users WHERE email_normalized='privacy-legacy@example.invalid';
