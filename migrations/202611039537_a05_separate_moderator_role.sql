-- A-05 / NEwBOO role parity: moderator is distinct from administrator.
-- A global moderator flag does NOT confer private reader G, wallet, mail,
-- site administration or book-team membership capabilities.
ALTER TABLE account_roles
    DROP CONSTRAINT account_roles_role_check,
    ADD CONSTRAINT account_roles_role_check
        CHECK (role IN ('author','moderator','administrator'));

ALTER TABLE account_role_events
    DROP CONSTRAINT account_role_events_role_check,
    ADD CONSTRAINT account_role_events_role_check
        CHECK (role IN ('author','moderator','administrator','state'));
