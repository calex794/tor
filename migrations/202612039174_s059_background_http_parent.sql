-- S059: preserve the server-issued HTTP parent ID across durable import retries.
-- A nullable fixed-size binary ID preserves existing jobs and avoids any public
-- user/session/source-data dimension. Roll out only through approved migrations.
ALTER TABLE chapter_import_jobs
    ADD COLUMN parent_trace_id BYTEA
    CONSTRAINT chapter_import_jobs_parent_trace_id_length
        CHECK (parent_trace_id IS NULL OR octet_length(parent_trace_id) = 16);
