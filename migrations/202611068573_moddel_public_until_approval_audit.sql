-- MOD-DEL-02: deletion REQUESTS must not deindex publicly approved books.
-- Legacy owner-start events recorded pending_deletion as the next lifecycle.
-- New events record actual unchanged Book lifecycle until site-moderator approval.
-- Keep book_ordinary_deletion_events_check (next_revision=previous_revision+1)
-- intact; only replace the separate workflow transition CHECK (check1).
-- This migration only changes audit validation; deploy is forbidden in this task.
ALTER TABLE book_ordinary_deletion_events
  DROP CONSTRAINT book_ordinary_deletion_events_check1;

ALTER TABLE book_ordinary_deletion_events
  ADD CONSTRAINT book_ordinary_deletion_events_check1
  CHECK (
    (action='start'
      AND (
        next_status='pending_deletion'
        OR (
          next_status=previous_status
          AND previous_status IN ('in_progress','completed','abandoned','on_hiatus')
        )
      )
    )
    OR
    (action='complete' AND next_status='ordinary_deleted'
      AND previous_status IN (
        'pending_deletion','in_progress','completed','abandoned','on_hiatus'
      )
    )
  );
