-- E-08: extend the *existing* immutable administrator mail access ledger.
-- No user-visible data or message bodies are copied into the audit.
-- Keep current direct-mail audits valid; add group/book IDs without foreign
-- keys, so deleting a chat never silently erases its access history.
ALTER TABLE public.social_mail_admin_access_audit
    ADD COLUMN group_id BIGINT CHECK (group_id IS NULL OR group_id > 0),
    ADD COLUMN book_id BIGINT CHECK (book_id IS NULL OR book_id > 0);

ALTER TABLE public.social_mail_admin_access_audit
    DROP CONSTRAINT social_mail_admin_access_audit_action_check,
    DROP CONSTRAINT social_mail_admin_access_audit_check;

ALTER TABLE public.social_mail_admin_access_audit
    ADD CONSTRAINT social_mail_admin_access_audit_action_check CHECK (
        action IN (
            'search_mail_users', 'list_user_mailbox', 'read_direct_conversation',
            'list_group_conversations', 'read_group_conversation',
            'list_book_rooms', 'read_book_room'
        )
    ),
    ADD CONSTRAINT social_mail_admin_access_audit_scope_check CHECK (
        (
            action IN ('search_mail_users','list_group_conversations','list_book_rooms')
            AND target_user_id IS NULL AND conversation_id IS NULL
            AND group_id IS NULL AND book_id IS NULL
        ) OR (
            action='list_user_mailbox' AND target_user_id IS NOT NULL
            AND conversation_id IS NULL AND group_id IS NULL AND book_id IS NULL
        ) OR (
            action='read_direct_conversation' AND conversation_id IS NOT NULL
            AND target_user_id IS NULL AND group_id IS NULL AND book_id IS NULL
        ) OR (
            action='read_group_conversation' AND group_id IS NOT NULL
            AND target_user_id IS NULL AND conversation_id IS NULL AND book_id IS NULL
        ) OR (
            action='read_book_room' AND book_id IS NOT NULL
            AND target_user_id IS NULL AND conversation_id IS NULL AND group_id IS NULL
        )
    );

-- Existing update/delete immutability trigger and migrator-only manual purge
-- automatically apply to all new audit actions. No change to retention period.
