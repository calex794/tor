-- E-08/S057 and blocking subset of E-06. PR-local draft migration, absent from main.
CREATE TABLE social_conversations (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK(id > 0),
  user_low BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  user_high BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_by BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  CHECK(user_low < user_high AND created_by IN(user_low,user_high)),
  UNIQUE(user_low, user_high)
);
CREATE INDEX social_conversations_low ON social_conversations(user_low, id DESC);
CREATE INDEX social_conversations_high ON social_conversations(user_high, id DESC);

CREATE TABLE social_user_blocks (
  blocker_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  blocked_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY(blocker_id, blocked_id),
  CHECK(blocker_id <> blocked_id)
);

CREATE TABLE social_messages (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK(id > 0),
  conversation_id BIGINT NOT NULL REFERENCES social_conversations(id) ON DELETE CASCADE,
  sender_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  body TEXT NOT NULL CHECK(char_length(body) >= 1 AND body = btrim(body)),
  command_key TEXT NOT NULL CHECK(command_key ~ '^[A-Za-z0-9_-]{43}
  created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  UNIQUE(sender_id, command_key),
  UNIQUE(conversation_id, id)
);
CREATE INDEX social_messages_page ON social_messages(conversation_id, id);
CREATE UNIQUE INDEX social_messages_html_retry ON social_messages(conversation_id,sender_id,client_message_id) WHERE client_message_id IS NOT NULL;

CREATE FUNCTION social_mail_identity_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_TABLE_NAME = 'social_messages' AND TG_OP = 'UPDATE' THEN
    RAISE EXCEPTION 'mail messages are immutable' USING ERRCODE='23514';
  ELSIF TG_TABLE_NAME = 'social_conversations' THEN
    IF ROW(NEW.user_low, NEW.user_high, NEW.created_by) IS DISTINCT FROM ROW(OLD.user_low, OLD.user_high, OLD.created_by) THEN
      RAISE EXCEPTION 'mail participants are immutable' USING ERRCODE='23514';
    END IF;
  ELSIF TG_TABLE_NAME = 'social_messages' THEN
    IF NOT EXISTS(SELECT 1 FROM social_conversations c WHERE c.id=NEW.conversation_id AND NEW.sender_id IN(c.user_low,c.user_high)) THEN
      RAISE EXCEPTION 'sender is not a mail participant' USING ERRCODE='23514';
    END IF;
  END IF;
  RETURN NEW;
END; $$;
CREATE TRIGGER social_conversation_identity BEFORE UPDATE ON social_conversations FOR EACH ROW EXECUTE FUNCTION social_mail_identity_guard();
CREATE TRIGGER social_message_identity BEFORE INSERT OR UPDATE ON social_messages FOR EACH ROW EXECUTE FUNCTION social_mail_identity_guard();
),
  client_message_id TEXT CHECK(client_message_id IS NULL OR (char_length(client_message_id) BETWEEN 1 AND 128 AND client_message_id ~ '^[A-Za-z0-9_.:-]+
  created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  UNIQUE(sender_id, command_key),
  UNIQUE(conversation_id, id)
);
CREATE INDEX social_messages_page ON social_messages(conversation_id, id);

CREATE FUNCTION social_mail_identity_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_TABLE_NAME = 'social_messages' AND TG_OP = 'UPDATE' THEN
    RAISE EXCEPTION 'mail messages are immutable' USING ERRCODE='23514';
  ELSIF TG_TABLE_NAME = 'social_conversations' THEN
    IF ROW(NEW.user_low, NEW.user_high, NEW.created_by) IS DISTINCT FROM ROW(OLD.user_low, OLD.user_high, OLD.created_by) THEN
      RAISE EXCEPTION 'mail participants are immutable' USING ERRCODE='23514';
    END IF;
  ELSIF TG_TABLE_NAME = 'social_messages' THEN
    IF NOT EXISTS(SELECT 1 FROM social_conversations c WHERE c.id=NEW.conversation_id AND NEW.sender_id IN(c.user_low,c.user_high)) THEN
      RAISE EXCEPTION 'sender is not a mail participant' USING ERRCODE='23514';
    END IF;
  END IF;
  RETURN NEW;
END; $$;
CREATE TRIGGER social_conversation_identity BEFORE UPDATE ON social_conversations FOR EACH ROW EXECUTE FUNCTION social_mail_identity_guard();
CREATE TRIGGER social_message_identity BEFORE INSERT OR UPDATE ON social_messages FOR EACH ROW EXECUTE FUNCTION social_mail_identity_guard();
)),
  created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  UNIQUE(sender_id, command_key),
  UNIQUE(conversation_id, id)
);
CREATE INDEX social_messages_page ON social_messages(conversation_id, id);

CREATE FUNCTION social_mail_identity_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_TABLE_NAME = 'social_messages' AND TG_OP = 'UPDATE' THEN
    RAISE EXCEPTION 'mail messages are immutable' USING ERRCODE='23514';
  ELSIF TG_TABLE_NAME = 'social_conversations' THEN
    IF ROW(NEW.user_low, NEW.user_high, NEW.created_by) IS DISTINCT FROM ROW(OLD.user_low, OLD.user_high, OLD.created_by) THEN
      RAISE EXCEPTION 'mail participants are immutable' USING ERRCODE='23514';
    END IF;
  ELSIF TG_TABLE_NAME = 'social_messages' THEN
    IF NOT EXISTS(SELECT 1 FROM social_conversations c WHERE c.id=NEW.conversation_id AND NEW.sender_id IN(c.user_low,c.user_high)) THEN
      RAISE EXCEPTION 'sender is not a mail participant' USING ERRCODE='23514';
    END IF;
  END IF;
  RETURN NEW;
END; $$;
CREATE TRIGGER social_conversation_identity BEFORE UPDATE ON social_conversations FOR EACH ROW EXECUTE FUNCTION social_mail_identity_guard();
CREATE TRIGGER social_message_identity BEFORE INSERT OR UPDATE ON social_messages FOR EACH ROW EXECUTE FUNCTION social_mail_identity_guard();
