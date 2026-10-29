-- E-03 / S012: private per-recipient read markers for canonical replies.
-- References only the original comment; bodies and spoilers are never copied.
CREATE TABLE discussion_reply_reads (
  recipient_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  reply_comment_id BIGINT NOT NULL REFERENCES discussion_comments(id) ON DELETE CASCADE,
  read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (recipient_user_id, reply_comment_id)
);
CREATE INDEX discussion_reply_reads_by_comment
  ON discussion_reply_reads (reply_comment_id);
