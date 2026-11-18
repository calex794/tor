// D-05 browser-level TOC regression: two archived FREE and two paid Layer
// rows. Runs isolated Chromium against real shipped JS, but no live payments,
// test user secrets, Postgres, S3 or public-reader fallback.
"use strict";
const assert = require("node:assert/strict");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const JS = fs.readFileSync(path.join(__dirname,
  "../crates/web/assets/newboo-layer-reader.mjs"), "utf8");
const fakePaid = [
  "export async function decryptPaidGrant() {",
  " return {versionId:'50',text:'Layer paid private fixture'};",
  "}",
  "export function readerBlocks(text) { return [text]; }",
].join("\n");
const html = `<!doctype html><html lang="ru"><head>
<meta charset="utf-8"></head><body>
<div data-layer-chapter-reader data-chapter-id="33"
     data-csrf-token="${"A".repeat(43)}" data-s3-origin="http://127.0.0.1:9000">
  <p data-layer-status></p><article data-layer-content></article>
  <details data-layer-toc><summary>Оглавление Layer</summary>
    <div data-layer-toc-modal>
      <button type="button" data-layer-toc-close>Закрыть</button>
      <div data-layer-toc-backdrop></div>
      <div data-layer-toc-scroll style="height:200px;overflow:auto">
        <ul data-layer-toc-list>
          <li data-toc-state="current" data-toc-no="3" data-toc-id="33">
          <span>Глава 3. Платная в архиве</span></li>
        </ul>
      </div>
      <p data-layer-toc-status></p>
    </div>
  </details>
</div>
<script type="module" src="/assets/newboo-layer-reader.mjs"></script>
</body></html>`;

function server() {
  const traffic = [];
  const httpServer = http.createServer((req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");
    const finish = (status, type, body) => {
      res.writeHead(status, { "content-type": type,
        "cache-control": "no-store", "x-content-type-options": "nosniff" });
      res.end(body);
    };
    if (url.pathname === "/") return finish(200, "text/html; charset=utf-8", html);
    if (url.pathname === "/assets/newboo-layer-reader.mjs") {
      return finish(200, "text/javascript", JS);
    }
    if (url.pathname === "/assets/newboo-paid-reader.mjs") {
      return finish(200, "text/javascript", fakePaid);
    }
    if (url.pathname === "/api/v1/reader/chapters/33/delivery-grants") {
      assert.equal(req.method, "POST");
      traffic.push("paid-grant");
      return finish(200, "application/json", JSON.stringify({schema:"test-only"}));
    }
    if (url.pathname === "/api/v1/reader/layer/33/toc/window") {
      assert.equal(req.method, "GET");
      const before = url.searchParams.get("before");
      const after = url.searchParams.get("after");
      assert.equal(url.searchParams.get("limit"), "36");
      assert.equal(before || after, "3.33");
      traffic.push(before ? "toc-before" : "toc-after");
      const items = before
        ? [
            { chapter_id:"11",chapter_no:1,
              title:"Архив 1 <img src=x onerror=alert(1)>",access_level:"archived_free" },
            { chapter_id:"22",chapter_no:2,
              title:"Архив 2 & остальные",access_level:"archived_free" },
          ]
        : [{ chapter_id:"44",chapter_no:4,
             title:"Оплаченная в Layer",access_level:"layer" }];
      return finish(200, "application/json", JSON.stringify({items,next_cursor:null}));
    }
    // Chromium may request /favicon.ico automatically. Do not treat this
    // browser housekeeping request as a reader/entitlement fallback.
    return finish(404, "text/plain", "not found");
  });
  return {httpServer,traffic};
}

async function verify(browser, origin, viewport, traffic) {
  const context = await browser.newContext({viewport});
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  try {
    const opened = await page.goto(origin + "/");
    assert.equal(opened.status(), 200);
    await page.waitForFunction(() =>
      (document.querySelector("[data-layer-status]")?.textContent || "")
        .includes("Защищённая загрузка завершена"));
    await page.locator("[data-layer-toc] summary").click();
    await page.waitForFunction(() =>
      document.querySelectorAll("[data-layer-toc-list] > li").length === 4);
    const rows = await page.locator("[data-layer-toc-list] > li").evaluateAll(list =>
      list.map(row => ({
        number: Number(row.dataset.tocNo),
        state: row.dataset.tocState,
        href: row.querySelector("a")?.getAttribute("href") || null,
        text: row.querySelector("a")?.textContent || "",
      })));
    const byNumber = Object.fromEntries(rows.map(row => [row.number,row]));
    assert.equal(byNumber[1].state, "archived_free");
    assert.equal(byNumber[1].href, "/read/layer/free/11");
    assert.equal(byNumber[2].state, "archived_free");
    assert.equal(byNumber[2].href, "/read/layer/free/22");
    assert.equal(byNumber[3].state, "current");
    assert.equal(byNumber[4].state, "layer");
    assert.equal(byNumber[4].href, "/read/layer/44");
    assert.equal(rows.length, 4);
    assert.equal(await page.locator("[data-layer-toc-list] img").count(), 0);
    assert(await page.locator("[data-layer-toc-list]").textContent()
      .then(text => text.includes("<img src=x onerror=alert(1)>")));
    assert(rows.every(row =>
      row.href === null || (!row.href.startsWith("/reader/") &&
        !row.href.includes("private/free-packs/") && !row.href.includes("X-Amz-"))));
    assert.deepEqual(errors, []);
    assert(traffic.includes("toc-before") && traffic.includes("toc-after"));
  } finally {
    await context.close();
  }
}

async function run() {
  const {httpServer,traffic} = server();
  await new Promise((resolve, reject) => {
    httpServer.once("error", reject);
    httpServer.listen(0, "127.0.0.1", resolve);
  });
  const addr = httpServer.address();
  const origin = "http://127.0.0.1:" + addr.port;
  let browser;
  try {
    browser = await chromium.launch({headless:true});
    await verify(browser, origin, {width:1440,height:900}, traffic);
    await verify(browser, origin, {width:390,height:844}, traffic);
  } finally {
    if (browser) await browser.close();
    await new Promise(resolve => httpServer.close(resolve));
  }
  console.log("D05 Layer TOC Chromium PASS: 2 archived FREE + 2 PAID, no public links");
}

run().catch(error => {
  console.error("D05 Layer TOC Chromium failed:", error.message);
  process.exitCode = 1;
});
