INSERT INTO users(id,email_normalized,display_name,password_hash)
OVERRIDING SYSTEM VALUE
SELECT id,'cover-'||id||'@example.invalid','CI Cover Owner','$argon2id$test'
  FROM generate_series(901,905) AS id;
INSERT INTO books(id,source_identity,target_language,title,visibility,owner_user_id,access_level)
SELECT id,'ci:cover:'||id,'ru','CI owned cover book','public',id,'basic'
  FROM generate_series(901,905) AS id;
