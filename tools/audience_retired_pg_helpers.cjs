'use strict';
// Shared acceptance helpers for an explicitly historical audience report.
// These fixture aggregates do not claim that the current runtime captured them.
const assert = require('node:assert/strict');
const {randomBytes} = require('node:crypto');

module.exports = function retiredAudience({sql, scalar, base}) {
  const positive = value => {
    assert.ok(Number.isSafeInteger(value) && value > 0);
    return value;
  };
  const count = value => {
    assert.ok(Number.isSafeInteger(value) && value >= 0);
    return value;
  };
  const ids = values => {
    assert.ok(values.length > 0);
    return values.map(positive).join(',');
  };
  async function csrf(page) {
    const token = (await page.context().cookies()).find(cookie => cookie.name === 'tor-dev-csrf')?.value;
    assert.ok(token, 'ordinary account forms issue a CSRF cookie');
    return token;
  }
  function oldScopes(users) {
    sql(`INSERT INTO rec_subjects(user_id,enabled,enabled_at,flow_epoch,flow_enabled_at)
      SELECT id,true,now()-interval '1 day',gen_random_uuid(),now()-interval '1 day'
      FROM users WHERE id IN(${ids(users)})`);
  }
  function scopeSnapshot(users) {
    return sql(`SELECT row_to_json(s)::text FROM rec_subjects s
      WHERE user_id IN(${ids(users)}) ORDER BY user_id`);
  }
  function history(rows) {
    const statements = rows.map(row => {
      const book = positive(row.book);
      const starts = count(row.starts ?? 0), completions = count(row.completions ?? 0);
      const clicks = count(row.clicks ?? 0), opens = count(row.opens ?? 0);
      if (row.source === 'recommendations') {
        const impressions = count(row.impressions ?? 0);
        assert.ok(clicks <= impressions, 'paired fixture clicks require a recorded impression');
        return `INSERT INTO rec_quality_daily
          (bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
           impressions,paired_clicks,unpaired_clicks,starts,completions,book_opens)
          VALUES((now() AT TIME ZONE 'UTC')::date,${book},'home','content','similar_by_content',1,
           'retained-browser-fixture','recommendations_v1',${impressions},${clicks},0,${starts},${completions},${opens})`;
      }
      assert.ok(['catalog','search','unattributed'].includes(row.source));
      return `INSERT INTO organic_discovery_quality_daily
        (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
        VALUES((now() AT TIME ZONE 'UTC')::date,${book},'${row.source}',${count(row.issued ?? 0)},
         ${clicks},${opens},${starts},${completions})`;
    });
    assert.ok(statements.length > 0);
    sql('BEGIN; '+statements.join('; ')+'; COMMIT');
  }
  function noCapture(users, books) {
    const actorIds = ids(users), bookIds = ids(books);
    const value = scalar(`SELECT
      (SELECT count(*) FROM rec_assignments WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM rec_first_conversions WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM rec_reader_signals WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM organic_discovery_touches WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM organic_discovery_first_conversions WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM organic_discovery_unattributed_reads WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM flow_visit_receipts r JOIN rec_subjects s USING(subject_id) WHERE s.user_id IN(${actorIds}))
     +(SELECT count(*) FROM flow_book_cohorts WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM reader_flow_read_receipts r JOIN rec_subjects s USING(subject_id) WHERE s.user_id IN(${actorIds}))
     +(SELECT count(*) FROM reader_flow_book_actions WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM reader_flow_activity_days WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM rec_dirty_buckets WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM organic_discovery_dirty WHERE book_id IN(${bookIds}))
     +(SELECT count(*) FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
       WHERE e.payload->>'user_id' IN(SELECT id::text FROM users WHERE id IN(${actorIds}))
         AND d.consumer IN('recommendation-feedback-v1','flow-read-attribution-v1','flow-read-activity-v1'))`);
    assert.equal(value, 0, 'ordinary navigation/C commands must not resume optional analytics');
  }
  function legacyBoundary() {
    // This suite proves legacy report arithmetic on the explicit 192 baseline.
    // Current 193 cleanup is exercised independently against the upgraded database.
    assert.equal(require('./legacy_analytics_fixture.cjs')(),
      process.env.LEGACY_ANALYTICS_DATABASE_URL);
  }
  async function marked(page, sections) {
    const retired = page.getByTestId('collection-retired');
    assert.equal(await retired.count(), 1);
    assert.equal(await retired.locator('[data-current-measurement="unavailable"]').count(), 1);
    const health = page.getByTestId(sections[0].replace(/-(current|previous)$/, '')+'-health');
    assert.match(await health.innerText(), /не подтверждает текущий сбор/);
    assert.doesNotMatch(await health.innerText(), /за последние 30 секунд/);
    for (const section of sections) {
      const values = page.getByTestId(section).locator('[data-metric]');
      assert.ok(await values.count() > 0, section+' retains historical metrics');
      assert.ok(await values.evaluateAll(nodes => nodes.every(node =>
        Boolean(node.closest('[data-measurement="legacy-consented"]')))),
      section+' must identify every numeric metric as historical');
    }
  }
  async function settings(page) {
    assert.equal((await page.goto(base+'/me/recommendations')).status(), 200);
    assert.equal(await page.locator(
      'form[action="/me/recommendations/consent"],form[action="/me/recommendations/flow-consent"]',
    ).count(), 0, 'ordinary settings must not ask for separate analytics consent');
  }
  async function navigate(page, fixture, source) {
    assert.ok(['catalog','search','recommendations'].includes(source));
    const target = source === 'recommendations' ? '/' : source === 'search'
      ? '/catalog?q='+encodeURIComponent(fixture.title) : '/catalog?sort=newest';
    assert.equal((await page.goto(base+target)).status(), 200);
    assert.equal(await page.locator('[data-rec-ticket],[data-organic-ticket]').count(), 0);
    const link = source === 'recommendations'
      ? page.locator('[data-rec-card]').filter({
        has:page.locator(`a[data-rec-link][href='/books/${positive(fixture.book)}']`),
      }).first().locator('a[data-rec-link]')
      : page.locator(`a.book-mini-card__link[href='/books/${positive(fixture.book)}']`).first();
    await link.scrollIntoViewIfNeeded();
    assert.ok(await link.isVisible(), 'ordinary rendered '+source+' link');
    await link.click();
    await page.waitForURL(base+`/books/${fixture.book}`);
  }
  async function rejectedLegacyCommands(page) {
    const ticket = randomBytes(32).toString('base64url');
    const result = await page.evaluate(async ({ticket,csrf}) => {
      const paths = ['/api/v1/recommendations/activate','/api/v1/discovery/activate',
        '/me/recommendations/consent','/me/recommendations/flow-consent'];
      const values = [];
      for (const path of paths) {
        for (let attempt=0; attempt<2; attempt++) {
          const response = await fetch(path, {
            method:'POST',credentials:'same-origin',
            headers:{'Content-Type':'application/x-www-form-urlencoded'},
            body:new URLSearchParams(path.includes('/activate')
              ? {ticket,csrf_token:csrf}
              : {csrf_token:csrf,enabled:'1',expected_revision:'0',command_key:ticket}),
          });
          values.push({status:response.status,cache:response.headers.get('cache-control'),
            robots:response.headers.get('x-robots-tag'),value:await response.json()});
        }
      }
      return values;
    }, {ticket,csrf:await csrf(page)});
    for (const response of result) {
      assert.equal(response.status, 410);
      assert.equal(response.cache, 'no-store');
      assert.match(response.robots, /noindex/);
      assert.match(response.robots, /nofollow/);
      assert.equal(response.value.error.code, 'COLLECTION_RETIRED');
    }
    return ticket;
  }
  return {oldScopes, scopeSnapshot, history, noCapture, legacyBoundary, marked, settings,
    navigate, rejectedLegacyCommands};
};
