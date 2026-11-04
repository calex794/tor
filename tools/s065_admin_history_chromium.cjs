// Real disposable PostgreSQL + Axum browser acceptance of private history.
"use strict";
const assert = require("node:assert/strict");
module.exports = async function historyAcceptance({
  browser, SITE, admin_id, admin_session, outsider_session,
  chapter_id, pg, count, contextFor, assertNoMoney,
}) {
  const book = count("SELECT book_id FROM chapters WHERE id=" + chapter_id);
  for (const [index, access] of ["free", "paid"].entries()) {
    const chapter = count("INSERT INTO chapters(book_id,chapter_no,title," +
      "visibility,access_level,price_amount) VALUES(" + book + "," +
      (950 + index) + ",'History browser','draft','" + access + "'," +
      (access === "paid" ? 175 : 0) + ") RETURNING id");
    const first = count("INSERT INTO chapter_versions(chapter_id,version_no," +
      "body_markdown) VALUES(" + chapter + ",1,'HISTORY-FIRST-PRIVATE') RETURNING id");
    const second = count("INSERT INTO chapter_versions(chapter_id,version_no," +
      "body_markdown) VALUES(" + chapter + ",2,'HISTORY-SECOND-PRIVATE') RETURNING id");
    pg("INSERT INTO chapter_working_drafts(chapter_id,version_id) VALUES(" +
      chapter + "," + second + ")");
    const endpoint = SITE + "/api/v1/reader/chapters/" + chapter + "/admin-history";
    const ctx = await contextFor(browser, admin_session);
    try {
      const page = await ctx.newPage();
      await page.setViewportSize(index === 0 ? { width: 1440, height: 900 }
        : { width: 390, height: 844 });
      const response = await page.goto(SITE + "/read/admin/" + chapter);
      assert.equal(response.status(), 200);
      const shell = await response.text();
      assert(!shell.includes("HISTORY-FIRST-PRIVATE"));
      assert(!shell.includes("HISTORY-SECOND-PRIVATE"));
      const article = page.locator("[data-admin-content]");
      const select = page.getByLabel("История редакций");
      await select.waitFor();
      assert.equal(await select.inputValue(), String(second));
      assert((await article.textContent()).includes("HISTORY-SECOND-PRIVATE"));
      await select.selectOption(String(first));
      await page.waitForFunction(() => document.querySelector("[data-admin-content]")
        .textContent.includes("HISTORY-FIRST-PRIVATE"));
      assert(!(await article.textContent()).includes("HISTORY-SECOND-PRIVATE"));
      assert.equal(await article.locator(".reader-save-button").count(), 0);
      await select.selectOption(String(second));
      await page.waitForFunction(() => document.querySelector("[data-admin-content]")
        .textContent.includes("HISTORY-SECOND-PRIVATE"));
      const csrf = await page.locator("[data-admin-chapter-reader]")
        .getAttribute("data-csrf-token");
      const headers = { origin: SITE, "x-csrf-token": csrf };
      const foreign = await ctx.request.post(endpoint + "/" + first, {
        headers: { ...headers, origin: "https://foreign.invalid" },
      });
      assert.equal(foreign.status(), 403);
      assert.equal((await foreign.body()).length, 0);
      const wrongCsrf = await ctx.request.post(endpoint + "/" + first, {
        headers: { ...headers, "x-csrf-token": "invalid" },
      });
      assert.equal(wrongCsrf.status(), 403);
      for (const [session, expected] of [[null, 401], [outsider_session, 403]]) {
        const other = await contextFor(browser, session);
        try {
          await other.addCookies([{ url: SITE, name: "tor-dev-csrf",
            value: csrf, sameSite: "Strict" }]);
          for (const suffix of ["", "/" + first]) {
            const denied = await other.request.post(endpoint + suffix, { headers });
            assert.equal(denied.status(), expected);
            assert.equal((await denied.body()).length, 0);
            assert((denied.headers()["cache-control"] || "").includes("no-store"));
          }
        } finally { await other.close(); }
      }
      // An already open tab must fail on its next version selection.
      pg("UPDATE account_roles SET is_active=FALSE WHERE user_id=" + admin_id +
        " AND role='administrator'");
      try {
        await select.selectOption(String(first));
        await page.locator("[data-admin-status]").filter({
          hasText: "Не удалось открыть редакцию.",
        }).waitFor();
        assert.equal(await article.textContent(), "");
      } finally {
        pg("UPDATE account_roles SET is_active=TRUE WHERE user_id=" + admin_id +
          " AND role='administrator'");
      }
      assertNoMoney();
      assert(count("SELECT count(*) FROM reader_admin_chapter_access_audit " +
        "WHERE admin_user_id=" + admin_id + " AND chapter_id=" + chapter +
        " AND version_id=" + first + " AND action='unsealed_body'") > 0);
    } finally { await ctx.close(); }
  }
};
