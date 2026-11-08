-- E-08/S057: indexed source-style public display-name resolution.
-- Existing names are not globally unique; keep the index non-unique and
-- fail closed on collisions rather than silently remapping identities.
CREATE INDEX users_public_display_name_fold_idx ON users (lower(display_name));
