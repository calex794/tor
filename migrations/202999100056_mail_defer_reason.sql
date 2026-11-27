-- Common dispatch diagnostics: no recipient, provider reply or message body.
ALTER TABLE mail_outbox ADD COLUMN defer_reason TEXT
    CHECK (defer_reason IN (
        'global_pace','recipient_cooldown','dns_unavailable',
        'transport_unavailable','activation_unavailable'
    ));
