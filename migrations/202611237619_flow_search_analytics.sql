-- FLOW-07: external search-engine aggregates. No reader identifiers or queries.
CREATE TABLE flow_seo_sources (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  provider text NOT NULL CHECK(provider IN ('google','yandex')),
  property text NOT NULL CHECK(length(property) BETWEEN 1 AND 256),
  origin text NOT NULL CHECK(length(origin) BETWEEN 9 AND 256),
  enabled boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE(provider,property,origin)
);
CREATE UNIQUE INDEX flow_seo_one_active_provider ON flow_seo_sources(provider) WHERE enabled;
CREATE TABLE flow_seo_jobs (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  source_id bigint NOT NULL REFERENCES flow_seo_sources(id) ON DELETE CASCADE,
  day date NOT NULL CHECK(day BETWEEN date '2000-01-01' AND date '2100-12-31'),
  state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','running','done','error')),
  epoch bigint NOT NULL DEFAULT 0 CHECK(epoch>=0),
  attempts integer NOT NULL DEFAULT 0 CHECK(attempts>=0),
  lease_until timestamptz,
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  last_attempt_at timestamptz,
  last_error text CHECK(last_error IN ('invalid','unavailable','authorization','rate_limited','stale','invalid_response')),
  UNIQUE(source_id,day),
  CHECK((state='running') = (lease_until IS NOT NULL))
);
CREATE INDEX flow_seo_job_claim ON flow_seo_jobs(source_id,next_attempt_at,day) WHERE state<>'done';
CREATE TABLE flow_seo_daily (
  source_id bigint NOT NULL REFERENCES flow_seo_sources(id) ON DELETE CASCADE,
  day date NOT NULL,
  measured_at timestamptz NOT NULL DEFAULT now(),
  status text NOT NULL CHECK(status IN ('available','no_data')),
  total_clicks bigint CHECK(total_clicks BETWEEN 0 AND 1000000000000),
  total_impressions bigint CHECK(total_impressions BETWEEN 0 AND 1000000000000),
  total_position double precision CHECK(total_position BETWEEN 0 AND 1000000000),
  provider_rows bigint NOT NULL CHECK(provider_rows BETWEEN 0 AND 1000000000000),
  omitted_rows bigint NOT NULL CHECK(omitted_rows BETWEEN 0 AND provider_rows),
  limit_reached boolean NOT NULL,
  PRIMARY KEY(source_id,day),
  CHECK((total_clicks IS NULL) = (total_impressions IS NULL)),
  CHECK(total_clicks<=total_impressions),
  CHECK(total_position IS NULL OR total_impressions IS NOT NULL)
);
CREATE TABLE flow_seo_pages (
  source_id bigint NOT NULL,
  day date NOT NULL,
  path text NOT NULL CHECK(path IN ('/','/catalog') OR path ~ '^/(books|reader)/[1-9][0-9]{0,18}$'),
  clicks bigint NOT NULL CHECK(clicks BETWEEN 0 AND 1000000000000),
  impressions bigint NOT NULL CHECK(impressions BETWEEN clicks AND 1000000000000),
  position double precision CHECK(position BETWEEN 0 AND 1000000000),
  PRIMARY KEY(source_id,day,path),
  FOREIGN KEY(source_id,day) REFERENCES flow_seo_daily(source_id,day) ON DELETE CASCADE
);
CREATE INDEX flow_seo_page_report ON flow_seo_pages(source_id,path,day);
