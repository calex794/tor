-- REC-13. Additive B/C/F schema submitted to I; no paid rights or historic DDL edits.
-- Unknown audiences are deliberately not eligible for recommendation.
CREATE TABLE book_recommendation_audiences (
    book_id bigint PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    audience text NOT NULL CHECK (audience IN ('general','adult','unreviewed')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision>0),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE book_recommendation_relations (
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    related_book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    PRIMARY KEY(book_id,related_book_id),
    CHECK(book_id<>related_book_id)
);
-- B-owned bounded public metadata port. Adult eligibility is not inferred.
CREATE VIEW rec_public_books WITH (security_barrier=true) AS
SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,b.created_at
FROM books b JOIN book_recommendation_audiences a ON a.book_id=b.id
WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
  AND a.audience='general';

CREATE TABLE rec_subjects (
    user_id bigint PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    subject_id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    enabled boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0),
    enabled_at timestamptz,
    changed_at timestamptz NOT NULL DEFAULT now(),
    CHECK (NOT enabled OR enabled_at IS NOT NULL)
);
CREATE TABLE rec_feedback_state (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    hidden boolean NOT NULL,
    revision bigint NOT NULL CHECK(revision>0),
    expires_at timestamptz NOT NULL DEFAULT now()+interval '30 days',
    PRIMARY KEY(subject_id,book_id)
);
CREATE TABLE rec_preference_commands (
    user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    command_key bytea NOT NULL CHECK(octet_length(command_key)=32),
    payload_hash bytea NOT NULL CHECK(octet_length(payload_hash)=32),
    result_revision bigint NOT NULL CHECK(result_revision>0),
    command_type text NOT NULL CHECK(command_type IN ('consent','feedback')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(user_id,command_key)
);
CREATE INDEX rec_preference_commands_expiry ON rec_preference_commands(created_at);

CREATE TABLE rec_generations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    status text NOT NULL DEFAULT 'building' CHECK(status IN ('building','validated','rejected')),
    cursor_book_id bigint NOT NULL DEFAULT 0 CHECK(cursor_book_id>=0),
    source_window_end timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    manifest_hash bytea CHECK(manifest_hash IS NULL OR octet_length(manifest_hash)=32)
);
CREATE UNIQUE INDEX rec_one_building_generation ON rec_generations((status)) WHERE status='building';
CREATE TABLE rec_candidate_edges (
    generation_id uuid NOT NULL REFERENCES rec_generations(id) ON DELETE CASCADE,
    seed_id bigint NOT NULL CHECK(seed_id>=0),
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    source text NOT NULL CHECK(source IN ('behavioral','content','popularity','related_books')),
    reason text NOT NULL CHECK(reason IN ('readers_also_read','similar_by_content','popular','trending','new_release','fallback_popular','manually_related')),
    score smallint NOT NULL CHECK(score BETWEEN 1 AND 1000),
    PRIMARY KEY(generation_id,seed_id,book_id,source)
);
CREATE INDEX rec_edges_serving ON rec_candidate_edges(generation_id,seed_id,source,score DESC,book_id);
CREATE TABLE rec_runtime_policy (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    active_generation uuid REFERENCES rec_generations(id) ON DELETE RESTRICT,
    previous_generation uuid REFERENCES rec_generations(id) ON DELETE RESTRICT,
    revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0),
    rollout_percent smallint NOT NULL DEFAULT 0 CHECK(rollout_percent IN (0,1,5,10,25,50,100)),
    hold boolean NOT NULL DEFAULT true
);
INSERT INTO rec_runtime_policy(singleton) VALUES(true);

CREATE TABLE rec_assignments (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    session_digest bytea NOT NULL CHECK(octet_length(session_digest)=32),
    ticket_digest bytea NOT NULL UNIQUE CHECK(octet_length(ticket_digest)=32),
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    surface text NOT NULL CHECK(surface IN ('home','book_detail','reader_end','catalog','library')),
    source text NOT NULL CHECK(source IN ('behavioral','content','popularity','related_books')),
    reason text NOT NULL CHECK(char_length(reason) BETWEEN 1 AND 64),
    position smallint NOT NULL CHECK(position BETWEEN 1 AND 20),
    generation_id uuid REFERENCES rec_generations(id) ON DELETE SET NULL,
    algorithm_version text NOT NULL DEFAULT 'recommendations_v1',
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL DEFAULT now()+interval '15 minutes',
    viewed_at timestamptz,
    clicked_at timestamptz,
    book_opened_at timestamptz,
    progress_observed_at timestamptz,
    started_at timestamptz,
    completed_at timestamptz,
    cohort_day date NOT NULL DEFAULT (now() AT TIME ZONE 'UTC')::date,
    CHECK(expires_at>created_at),
    CHECK(viewed_at IS NULL OR viewed_at>=created_at),
    CHECK(clicked_at IS NULL OR clicked_at>=created_at)
);
CREATE INDEX rec_assignment_subject_book ON rec_assignments(subject_id,book_id,clicked_at DESC) WHERE clicked_at IS NOT NULL;
CREATE INDEX rec_assignment_subject_budget ON rec_assignments(subject_id,created_at);
CREATE INDEX rec_assignment_cohort ON rec_assignments(cohort_day,book_id);
CREATE INDEX rec_assignment_expiry ON rec_assignments(created_at);
CREATE TABLE rec_attribution_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    assignment_id uuid NOT NULL REFERENCES rec_assignments(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK(kind IN ('impression','click','book_opened','chapter_started','chapter_finished')),
    source_event_id uuid UNIQUE,
    occurred_at timestamptz NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    trust text NOT NULL CHECK(trust IN ('client_observed','server_navigation','domain_committed')),
    UNIQUE(assignment_id,kind),
    CHECK (
      (kind='impression' AND trust='client_observed' AND source_event_id IS NULL)
      OR (kind IN ('click','book_opened') AND trust='server_navigation' AND source_event_id IS NULL)
      OR (kind IN ('chapter_started','chapter_finished') AND trust='domain_committed' AND source_event_id IS NOT NULL)
    )
);
CREATE TABLE rec_first_conversions (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK(kind IN ('chapter_started','chapter_finished')),
    assignment_id uuid NOT NULL REFERENCES rec_assignments(id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL UNIQUE,
    occurred_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,book_id,kind)
);
CREATE TABLE rec_reader_signals (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    occurred_at timestamptz NOT NULL,
    source_event_id uuid NOT NULL UNIQUE,
    PRIMARY KEY(subject_id,book_id)
);
CREATE INDEX rec_reader_signals_recent ON rec_reader_signals(subject_id,occurred_at DESC);
CREATE TABLE rec_dirty_buckets (
    bucket_date date NOT NULL,
    book_id bigint NOT NULL,
    PRIMARY KEY(bucket_date,book_id)
);
CREATE TABLE rec_quality_daily (
    bucket_date date NOT NULL,
    book_id bigint NOT NULL,
    surface text NOT NULL,
    source text NOT NULL,
    reason text NOT NULL,
    position smallint NOT NULL CHECK(position BETWEEN 1 AND 20),
    generation text NOT NULL,
    algorithm_version text NOT NULL,
    impressions bigint NOT NULL CHECK(impressions>=0),
    paired_clicks bigint NOT NULL CHECK(paired_clicks>=0 AND paired_clicks<=impressions),
    unpaired_clicks bigint NOT NULL CHECK(unpaired_clicks>=0),
    starts bigint NOT NULL CHECK(starts>=0),
    completions bigint NOT NULL CHECK(completions>=0),
    refreshed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(bucket_date,book_id,surface,source,reason,position,generation,algorithm_version)
);
CREATE TABLE rec_worker_health (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    last_success timestamptz NOT NULL
);
-- C-owned trusted first progress fact. Only C validated CAS inserts here.
CREATE TABLE reader_chapter_starts (
    user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    chapter_id bigint NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    first_version_id bigint NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now(),
    source_event_id uuid NOT NULL UNIQUE,
    PRIMARY KEY(user_id,book_id,chapter_id),
    FOREIGN KEY(chapter_id,first_version_id) REFERENCES chapter_versions(chapter_id,id)
);
CREATE INDEX reader_chapter_starts_book ON reader_chapter_starts(user_id,book_id,started_at);
