// Named test identities are assigned after real two-field browser registration.
const assert = require('node:assert/strict');

async function setFixtureDisplayName(page, displayName) {
  const base = new URL(page.url()).origin;
  const response = await page.request.get(base + '/api/v1/me');
  assert.equal(response.status(), 200);
  const profile = await response.json();
  assert.ok(profile.csrf_token);
  const changed = await page.request.patch(base + '/api/v1/me', {
    headers: { Origin: base },
    data: {
      display_name: displayName,
      expected_revision: profile.revision,
      csrf_token: profile.csrf_token,
    },
  });
  assert.equal(changed.status(), 200, 'fixture name assignment failed');
  assert.ok((await changed.json()).revision > profile.revision);
}

module.exports = { setFixtureDisplayName };
