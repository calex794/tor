-- E-03: bounded canonical comment mentions, without duplicated comment bodies.
-- Handles are private until friend relationships have been ported.
CREATE TABLE discussion_mention_handles (
  user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  handle TEXT NOT NULL UNIQUE CHECK (
    char_length(handle) BETWEEN 3 AND 32
    AND handle ~ '^[a-z0-9_.]+$'
  ),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE discussion_comment_mentions (
  comment_id BIGINT NOT NULL REFERENCES discussion_comments(id) ON DELETE CASCADE,
  recipient_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(comment_id, recipient_user_id)
);
CREATE INDEX discussion_mentions_recipient_comment
  ON discussion_comment_mentions(recipient_user_id, comment_id DESC);
