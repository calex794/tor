-- MAIL-01h1: approved local Postbox budget, not the provider's billing account.
-- Disabled until the operator verifies current billing scope and usage.
CREATE TABLE mail_cost_policy (
    singleton BOOLEAN PRIMARY KEY CHECK (singleton),
    enabled BOOLEAN NOT NULL DEFAULT false,
    warning_1 BIGINT NOT NULL DEFAULT 10000,
    warning_2 BIGINT NOT NULL DEFAULT 30000,
    news_pause BIGINT NOT NULL DEFAULT 40000,
    monthly_limit BIGINT NOT NULL DEFAULT 50000,
    CHECK (0 < warning_1 AND warning_1 <= warning_2
        AND warning_2 <= news_pause AND news_pause < monthly_limit)
);
INSERT INTO mail_cost_policy(singleton) VALUES (true);

-- No recipient, IP, user, message or token. Keep monthly aggregate even when
-- the 48h short-term reservation ledger or account rows are pruned.
-- No implicit first-use initialization, including after recovery.
CREATE TABLE mail_cost_months (
    month_start DATE PRIMARY KEY CHECK (EXTRACT(DAY FROM month_start)=1),
    free_allowance BIGINT NOT NULL DEFAULT 0 CHECK (free_allowance BETWEEN 0 AND 2000),
    baseline_kopecks BIGINT NOT NULL CHECK (baseline_kopecks>=0),
    rate_per_thousand BIGINT NOT NULL CHECK (rate_per_thousand>=8032),
    reserved_attempts BIGINT NOT NULL DEFAULT 0 CHECK (reserved_attempts>=0)
);
