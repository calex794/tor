'use strict';
// FLOW-02/04 assertions share the real hourly browser/server/PG journey.
const assert = require('node:assert/strict');
function helpers(sql,json) {
  function counts(from) {
    return json(`SELECT json_agg(row_to_json(r) ORDER BY r.surface) FROM (
      SELECT s.surface,COALESCE(SUM(f.html_requests),0)::bigint AS count
      FROM (VALUES(0),(1),(2),(3))s(surface)
      LEFT JOIN ril_flow_observed_hourly f ON f.surface_code=s.surface
        AND f.bucket>=to_timestamp(${from}) GROUP BY s.surface)r`).map(r=>r.count);
  }
  function history(anchor) {
    const epochs=[3,4,171,3000].map(h=>anchor-h*3600);
    const predicate='bucket IN('+epochs.map(e=>`to_timestamp(${e})`).join(',')+')';
    const saved=sql(`SELECT extract(epoch FROM bucket)::bigint,surface_code,
      acquisition_hint,html_requests FROM ril_flow_observed_hourly WHERE ${predicate}`);
    const [current,empty,previous,old]=epochs;
    sql(`BEGIN; DELETE FROM ril_flow_observed_hourly WHERE ${predicate};
      INSERT INTO ril_flow_observed_hourly VALUES
        (to_timestamp(${current}),0,0,2),(to_timestamp(${current}),0,1,3),
        (to_timestamp(${current}),1,3,0),(to_timestamp(${previous}),2,3,7),
        (to_timestamp(${old}),3,3,11); COMMIT`);
    return {current,empty,restore() {
      const rows=saved?saved.split('\n').map(line=>{
        const fields=line.split('|');assert.equal(fields.length,4);
        fields.forEach(f=>assert.match(f,/^\d+$/));
        return `INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp(${fields[0]}),${fields.slice(1).join(',')});`;
      }).join(''):'';
      sql(`BEGIN; DELETE FROM ril_flow_observed_hourly WHERE ${predicate}; ${rows} COMMIT`);
    }};
  }
  async function report(page,days,zone='UTC') {
    const root=page.getByTestId('audience-observed');
    assert.equal(await root.count(),1);
    assert.ok((await root.innerText()).includes(zone));
    const bounds={};
    for(const kind of ['current','previous']) {
      const section=page.getByTestId('audience-observed-'+kind);
      const times=await section.locator('time[datetime]').evaluateAll(nodes=>
        nodes.map(n=>Date.parse(n.getAttribute('datetime'))/1000));
      assert.equal(times.length,2);const[from,until]=times;
      assert.equal(until-from,days*86400);assert.equal(from%3600,0);
      bounds[kind]=times;
      const query=`bucket>=to_timestamp(${from}) AND bucket<to_timestamp(${until})`;
      const totals=json(`SELECT row_to_json(r) FROM (SELECT SUM(html_requests)::text AS n,
        count(DISTINCT bucket)::int AS hours FROM ril_flow_observed_hourly WHERE ${query})r`);
      const shown=(await section.locator('[data-metric="observed-total"]').innerText()).trim();
      if(totals.n===null)assert.ok(!/^\d+$/.test(shown));else assert.equal(shown,totals.n);
      assert.equal((await section.locator('[data-metric="observed-hours"]').innerText()).trim(),String(totals.hours));
      for(const [field,attr]of[['surface_code','surface'],['acquisition_hint','source']]) {
        const expected=json(`SELECT json_agg(row_to_json(r) ORDER BY r.code) FROM (
          SELECT s.code,SUM(f.html_requests)::text AS n FROM (VALUES(0),(1),(2),(3))s(code)
          LEFT JOIN ril_flow_observed_hourly f ON f.${field}=s.code AND ${query}
          GROUP BY s.code)r`);
        for(const row of expected) {
          const value=(await section.locator(`[data-observed-${attr}="${row.code}"]`).innerText()).trim();
          if(row.n===null)assert.ok(!/^\d+$/.test(value));else assert.equal(value,row.n);
        }
      }
    }
    assert.equal(bounds.previous[1],bounds.current[0]);
    const table=page.getByTestId('audience-observed-table');
    const epochs=await table.locator('[data-observed-hour]').evaluateAll(rows=>rows.map(r=>Number(r.dataset.observedHour)));
    assert.equal(epochs.length,Math.min(days*24,168));
    assert.equal(new Set(epochs).size,epochs.length);
    assert.equal(epochs.at(-1),bounds.current[1]-3600);
    assert.equal(await page.getByTestId('audience-observed-week').locator('tbody tr').count(),7);
    assert.equal(await page.locator('select[name="time_zone"]').inputValue(),zone);
    return bounds;
  }
  return {counts,history,report};
}
module.exports={helpers};
