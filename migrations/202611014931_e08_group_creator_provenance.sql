-- E-08: creation provenance is not a lifecycle owner.
-- An original creator can transfer administration and delete the account
-- without deleting the group's later members and their shared conversation.
-- The old DDL is immutable SQLx migration history; change the FK only here.
ALTER TABLE social_group_conversations
    DROP CONSTRAINT social_group_conversations_created_by_fkey;

ALTER TABLE social_group_conversations
    ALTER COLUMN created_by DROP NOT NULL;

ALTER TABLE social_group_conversations
    ADD CONSTRAINT social_group_conversations_created_by_fkey
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

COMMENT ON COLUMN social_group_conversations.created_by IS
    'Historical founder only; null when founder account is deleted. Current creator role lives in social_group_members.';
