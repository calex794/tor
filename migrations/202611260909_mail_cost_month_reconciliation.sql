-- MAIL-01j: auditable, conservative reconciliation of stopped mail budgets.
-- Existing month rows and the policy are not changed by this migration.
CREATE TABLE mail_cost_month_reconciliation_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id > 0),
    month_start DATE NOT NULL CHECK (EXTRACT(DAY FROM month_start)=1),
    old_revision BIGINT NOT NULL CHECK (old_revision > 0),
    new_revision BIGINT NOT NULL UNIQUE CHECK (new_revision=old_revision+1),
    evidence_sha256 BYTEA NOT NULL CHECK (octet_length(evidence_sha256)=32
        AND evidence_sha256<>decode(repeat('00',32),'hex')),
    old_free_allowance BIGINT,
    old_baseline_kopecks BIGINT,
    old_rate_per_thousand BIGINT,
    old_reserved_attempts BIGINT,
    new_free_allowance BIGINT NOT NULL CHECK (new_free_allowance BETWEEN 0 AND 2000),
    new_baseline_kopecks BIGINT NOT NULL CHECK (new_baseline_kopecks>=0),
    new_rate_per_thousand BIGINT NOT NULL CHECK (new_rate_per_thousand>=8032),
    new_reserved_attempts BIGINT NOT NULL CHECK (new_reserved_attempts>=0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (
        (old_free_allowance IS NULL AND old_baseline_kopecks IS NULL
          AND old_rate_per_thousand IS NULL AND old_reserved_attempts IS NULL)
        OR
        (old_free_allowance IS NOT NULL AND old_baseline_kopecks IS NOT NULL
          AND old_rate_per_thousand IS NOT NULL AND old_reserved_attempts IS NOT NULL
          AND old_free_allowance BETWEEN 0 AND 2000 AND old_baseline_kopecks>=0
          AND old_rate_per_thousand>=8032 AND old_reserved_attempts>=0
          AND new_free_allowance<=old_free_allowance
          AND new_baseline_kopecks>=old_baseline_kopecks
          AND new_rate_per_thousand>=old_rate_per_thousand
          AND new_reserved_attempts>=old_reserved_attempts)
    )
);
CREATE TRIGGER mail_cost_month_reconciliation_audit_immutable
BEFORE UPDATE OR DELETE OR TRUNCATE ON mail_cost_month_reconciliation_audit
FOR EACH STATEMENT EXECUTE FUNCTION mail_cost_admin_audit_immutable();
