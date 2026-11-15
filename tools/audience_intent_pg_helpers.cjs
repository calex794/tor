// Existing product actions in the real browser, not synthetic analytics events.
const assert = require('node:assert/strict');
const metrics = ['library-links','library-accounts','follow-links','follow-accounts'];
async function snapshot(page) {
  const result = {};
  for (const cohort of ['total','current','previous']) {
    result[cohort] = {};
    for (const metric of metrics) {
      const text = await page.getByTestId('audience-intents-'+cohort).locator(`[data-metric="${metric}"]`).innerText();
      assert.match(text,/^\d+$/,'available product snapshot must be numerical');
      result[cohort][metric] = Number(text);
    }
  }
  return result;
}
async function choices(page, book, csrf, remove=false) {
  await page.evaluate(async ({book,csrf,remove}) => {
    for (const route of [`/me/library/${book}/${remove?'remove':'save'}`,`/me/books/${book}/${remove?'unfollow':'follow'}`]) {
      const response = await fetch(route, {method:'POST',credentials:'same-origin',
        headers:{'Content-Type':'application/x-www-form-urlencoded'},body:new URLSearchParams({csrf_token:csrf})});
      if (response.status !== 200 || !response.redirected) throw new Error('product choice failed: '+response.status);
    }
  },{book,csrf,remove});
}
function delta(before, after, amount, cohorts=['total','current']) {
  for (const cohort of cohorts) for (const metric of metrics) {
    assert.equal(after[cohort][metric],before[cohort][metric]+amount,`${cohort}/${metric}: product row counted once`);
  }
}
module.exports={snapshot,choices,delta};
