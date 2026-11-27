-- MAIL-01f1: conservative provider-wide rolling send budget.
-- Ledger rows deliberately survive deletion of accounts and source events.
CREATE TABLE mail_send_budget_guard (
    singleton BOOLEAN PRIMARY KEY CHECK (singleton)
);
INSERT INTO mail_send_budget_guard(singleton) VALUES (true);

CREATE TABLE mail_send_reservations (
    outbox_id BIGINT NOT NULL CHECK (outbox_id > 0),
    generation INTEGER NOT NULL CHECK (generation > 0),
    purpose TEXT NOT NULL CHECK (purpose IN (
        'registration_confirmation', 'support_reply', 'project_news'
    )),
    recipient_key BYTEA NOT NULL CHECK (octet_length(recipient_key) = 32),
    registration_ip_key BYTEA CHECK (octet_length(registration_ip_key) = 32),
    reserved_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (outbox_id, generation),
    CHECK (purpose = 'registration_confirmation' OR registration_ip_key IS NULL)
);

CREATE INDEX mail_send_reservations_time ON mail_send_reservations(reserved_at);
