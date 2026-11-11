-- F-SURPRISE-04A: independent randomized control, additive/default OFF.
-- The existing F decision row already stores a user/day bounded receipt.
-- Never add a raw click, visit, page, device or interaction log for the pilot.
ALTER TABLE surprise_runtime_policy
    ADD COLUMN experiment_enabled boolean NOT NULL DEFAULT false;

ALTER TABLE surprise_daily_decisions
    ADD COLUMN experiment_arm text
        CHECK(experiment_arm IN ('control','gentle','standard'));

CREATE INDEX surprise_experiment_daily_arm
    ON surprise_daily_decisions(utc_day,experiment_arm)
    WHERE experiment_arm IS NOT NULL;

COMMENT ON COLUMN surprise_runtime_policy.experiment_enabled IS
    'Default OFF. HMAC-assigned F-SURPRISE control/gentle/standard pilot, independent of global enable and HTTP gates.';
COMMENT ON COLUMN surprise_daily_decisions.experiment_arm IS
    'Stable server-side pilot arm per account, saved only in the already bounded F daily decision. NULL means non-experiment.';
