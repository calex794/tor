-- REC-13: prevent replay of erased preferences across consent and TTL epochs.
-- The epoch carries no per-book preference; it is only a monotonic CAS fence.
-- An epoch of 1,000,000 and a per-book revision below 1e9 remain JSON-safe.
ALTER TABLE rec_subjects
  ADD COLUMN feedback_epoch bigint NOT NULL DEFAULT 0
  CHECK (feedback_epoch BETWEEN 0 AND 1000000);

ALTER TABLE rec_feedback_state
  ADD CONSTRAINT rec_feedback_revision_stride
  CHECK (revision BETWEEN 1 AND 999999999);

-- Expiration scanning must not require a full scan of personal preferences.
CREATE INDEX rec_feedback_expiration ON rec_feedback_state(expires_at,subject_id);
