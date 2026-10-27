// H/S034: actual Chromium, server-generated paid shell and real private MinIO GET.
// The A/D/B/G HTTP contract is tested in Rust before this ephemeral fixture.
// No grant, signed link, scoped DEK, cookie or plaintext is logged or archived.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const fixturePath = process.env.TOR_PAID_GRANT_FIXTURE_PATH;
assert(fixturePath && process.env.RUNNER_TEMP &&
  path.resolve(fixturePath).startsWith(path.resolve(process.env.RUNNER_TEMP) + path.sep),
"Fixture must stay inside temporary CI directory");

const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
const grant = fixture.grant;
const chapter = grant.chapter_id;
const purchase = fixture.purchase_id;
const pageUrl = SITE + "/read/paid/" + chapter + "?purchase_id=" + purchase;
const data = fixture.reader_html;
const csp = fixture.reader_csp;
const script = fs.readFileSync("crates/web/assets/newboo-paid-reader.mjs");
// Serve the EXACT two stylesheets used by pages::layout and
// pages::newboo_stylesheet, not reader-only CSS as both URLs.
const siteCss = fs.readFileSync("crates/web/assets/site.css");
const orderedNewbooFiles = [
  "newboo-port.css",
  "newboo-book-detail.css",
  "newboo-reader.css",
  "newboo-cover-picker.css",
];
// Extract the actual Rust handler's include_str! order, so future changes
// cannot silently turn this Chromium smoke into a stale visual fixture.
const pageSource = fs.readFileSync("crates/web/src/pages.rs", "utf8");
const start = pageSource.indexOf("pub async fn newboo_stylesheet()");
const end = pageSource.indexOf("pub async fn paid_reader_script()", start);
assert(start >= 0 && end > start, "NEwBOO Rust CSS handler missing");
const sourceOrder = [...pageSource.slice(start, end).matchAll(
  /include_str!\("\.\.\/assets\/([^"]+\.css)"\)/g
)].map((match) => match[1]);
assert.deepEqual(sourceOrder, orderedNewbooFiles,
  "Rust pages::newboo_stylesheet CSS list changed: update visual baseline");
const newbooCss = orderedNewbooFiles
  .map((file) => fs.readFileSync(path.join("crates/web/assets", file), "utf8"))
  .join("\n");
const brandLogo = fs.readFileSync("crates/web/assets/brand/pandaolate-header.png");
const brandLogoDark = fs.readFileSync("crates/web/assets/brand/pandaolate-header-dark.png");
const screenshotRoot = process.env.TOR_PAID_READER_SHOTS;
assert(screenshotRoot && path.resolve(screenshotRoot).startsWith(
  path.resolve(process.env.RUNNER_TEMP) + path.sep
), "Visual screenshots must remain in the disposable CI temp directory");
fs.mkdirSync(screenshotRoot, { recursive: true });
assert(data.includes('<html lang="ru">') &&
  data.includes('<body class="reader-standalone">') &&
  data.includes('<main id="main" class="reader-standalone__main">') &&
  data.includes('<link rel="stylesheet" href="/assets/site.css">') &&
  data.includes('<link rel="stylesheet" href="/assets/newboo.css">') &&
  data.includes("data-paid-chapter-reader") &&
  !data.includes('class="site-header"') &&
  !data.includes('class="site-footer"') &&
  !data.includes('class="site-logo"'),
"Actual Rust reader must use NEwBOO standalone chrome, without global header/footer");
assert(data.includes("data-paid-chapter-reader") &&
  !data.includes(fixture.expected_markdown) && !data.includes(grant.content.dek_b64u));
assert(csp.includes("script-src 'self'") && csp.includes(S3));
assert.equal(grant.version_id, fixture.purchased_version_id);

const csrf = data.match(/data-csrf-token="([A-Za-z0-9_-]{43})"/)?.[1];
assert(csrf, "Real Rust shell must include a valid CSRF token");
const send = (reply, status, body, type, extra = {}) => {
  reply.writeHead(status, {
    "content-type": type,
    "cache-control": "private, no-store",
    "referrer-policy": "no-referrer",
    "x-content-type-options": "nosniff",
    ...extra,
  });
  reply.end(body);
};
const readBody = (req) => new Promise((resolve, reject) => {
  let raw = "";
  req.on("data", (chunk) => {
    raw += chunk.toString("utf8");
    if (raw.length > 512) {
      req.destroy();
      reject(new Error("Unexpectedly large grant request"));
    }
  });
  req.on("end", () => resolve(raw));
  req.on("error", reject);
});
let grantCalls = 0;
let saved = null;
let markedComplete = false;
let completionPosts = 0;
const server = http.createServer(async (req, reply) => {
  try {
    const uri = new URL(req.url, SITE);
    if (req.method === "GET" && uri.pathname === "/read/paid/" + chapter) {
      assert.equal(uri.searchParams.get("purchase_id"), purchase);
      send(reply, 200, data, "text/html; charset=utf-8",
        { "content-security-policy": csp });
    } else if (req.method === "GET" && uri.pathname === "/assets/newboo-paid-reader.mjs") {
      send(reply, 200, script, "application/javascript; charset=utf-8",
        { "cache-control": "public, max-age=3600" });
    } else if (req.method === "GET" && uri.pathname === "/assets/site.css") {
      send(reply, 200, siteCss, "text/css; charset=utf-8",
        { "cache-control": "public, max-age=3600" });
    } else if (req.method === "GET" && uri.pathname === "/assets/newboo.css") {
      send(reply, 200, newbooCss, "text/css; charset=utf-8",
        { "cache-control": "public, max-age=3600" });
    } else if (req.method === "GET" && uri.pathname === "/assets/brand/pandaolate-header.png") {
      send(reply, 200, brandLogo, "image/png", { "cache-control": "public, max-age=3600" });
    } else if (req.method === "GET" &&
      uri.pathname === "/assets/brand/pandaolate-header-dark.png") {
      send(reply, 200, brandLogoDark, "image/png",
        { "cache-control": "public, max-age=3600" });
    } else if (req.method === "POST" &&
      uri.pathname === "/api/v1/reader/chapters/" + chapter + "/delivery-grants") {
      assert.equal(req.headers.origin, SITE);
      assert.equal(req.headers["x-csrf-token"], csrf);
      assert.equal(req.headers["content-type"], "application/json");
      assert.deepEqual(JSON.parse(await readBody(req)), { purchase_id: purchase });
      grantCalls++;
      send(reply, 200, JSON.stringify(grant), "application/json");
    } else if (uri.pathname === "/api/v1/me/progress/purchased/complete" &&
      req.method === "GET") {
      assert.equal(uri.searchParams.get("chapter_id"), chapter);
      assert.equal(uri.searchParams.get("purchase_id"), purchase);
      assert.equal(uri.searchParams.get("version_id"), grant.version_id);
      send(reply, 200, JSON.stringify({
        available: true, completed: markedComplete,
      }), "application/json");
    } else if (uri.pathname === "/api/v1/me/progress/purchased/complete" &&
      req.method === "POST") {
      assert.equal(req.headers.origin, SITE);
      assert.equal(req.headers["x-csrf-token"], csrf);
      const body = JSON.parse(await readBody(req));
      assert.equal(body.purchase_id, purchase);
      assert.equal(body.chapter_id, chapter);
      assert.equal(body.version_id, grant.version_id);
      assert.equal(body.expected_revision, 1);
      assert.equal(body.block_id, fixture.expected_markdown.split("\n\n").length);
      assert.equal(body.char_offset, fixture.expected_markdown.split("\n\n").at(-1).length);
      const aad = Buffer.from(grant.content.aad_b64u, "base64url").toString("utf8").split("\n");
      assert.equal(body.plaintext_sha256, aad[4]);
      completionPosts++;
      const old = markedComplete;
      markedComplete = true;
      send(reply, 200, JSON.stringify({
        status: old ? "already_completed" : "completed",
      }), "application/json");
    } else if (uri.pathname === "/api/v1/me/progress/purchased" &&
      req.method === "GET") {
      assert.equal(uri.searchParams.get("chapter_id"), chapter);
      assert.equal(uri.searchParams.get("purchase_id"), purchase);
      assert.equal(uri.searchParams.get("version_id"), grant.version_id);
      send(reply, 200, JSON.stringify({
        chapter_id: chapter, version_id: grant.version_id, revision: 0,
        block_id: null, char_offset: null,
      }), "application/json");
    } else if (uri.pathname === "/api/v1/me/progress/purchased" &&
      req.method === "PUT") {
      assert.equal(req.headers.origin, SITE);
      assert.equal(req.headers["x-csrf-token"], csrf);
      saved = JSON.parse(await readBody(req));
      send(reply, 200, JSON.stringify({ revision: 1, version_id: grant.version_id }),
        "application/json");
    } else {
      send(reply, 404, "", "text/plain");
    }
  } catch {
    // Never echo request/grant/secret in CI; the calling browser will fail closed.
    send(reply, 503, "", "text/plain");
  }
});

function signatureExpiry(url) {
  const date = url.searchParams.get("X-Amz-Date");
  const seconds = Number(url.searchParams.get("X-Amz-Expires"));
  assert(/^[0-9]{8}T[0-9]{6}Z$/.test(date) && seconds === 30);
  return Date.UTC(
    +date.slice(0, 4), +date.slice(4, 6) - 1, +date.slice(6, 8),
    +date.slice(9, 11), +date.slice(11, 13), +date.slice(13, 15)
  ) + seconds * 1000;
}

async function run() {
  assert.equal(new URL(grant.content.ciphertext_url).origin, S3);
  const short = new URL(fixture.expiry_probe_url);
  assert.equal(short.origin, S3);
  assert.equal(short.searchParams.get("X-Amz-Expires"), "30");
  const initiallySigned = await fetch(short);
  assert.equal(initiallySigned.status, 200, "30s signed GET must work before expiry");
  await initiallySigned.arrayBuffer();

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(3098, "127.0.0.1", resolve);
  });
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [device, width, height] of [
      ["desktop", 1440, 900], ["mobile", 390, 844],
    ]) {
      const page = await browser.newPage({ viewport: { width, height },
        colorScheme: "light" });
      let realS3 = 0;
      const loadedAssets = new Set();
      page.on("response", (response) => {
        const url = new URL(response.url());
        if (url.origin === S3 && url.pathname.endsWith(".enc")) {
          if (response.status() === 200) realS3++;
        }
        if (url.origin === SITE && response.status() === 200) {
          loadedAssets.add(url.pathname);
        }
      });
      try {
        const response = await page.goto(pageUrl, { waitUntil: "networkidle" });
        assert.equal(response.status(), 200);
        await page.locator("[data-paid-status]")
          .getByText("Защищённая загрузка завершена", { exact: false })
          .waitFor({ timeout: 20000 });
        const chunks = await page.locator(".reader-piece__text").allTextContents();
        assert(chunks.join("\n\n") === fixture.expected_markdown,
          "Decrypted synthetic fixture differs (do not log paid plaintext)");
        assert.equal(realS3, 1, "Browser must fetch ciphertext from actual MinIO");
        // These are the original PostgreSQL titles in real Rust HTML, not
        // the chapter IDs, last-published V2, or test-only page labels.
        assert.equal(await page.locator(".reader-topbar__book").textContent(),
          fixture.expected_book_title);
        const chapterLabel = "Глава " + fixture.expected_chapter_no + ". " +
          fixture.expected_chapter_title;
        assert.equal(await page.locator(".reader-topbar__chapter").textContent(), chapterLabel);
        assert.equal(await page.locator("#reader-title").textContent(), chapterLabel);
        assert.equal(await page.locator(".reader-page__meta").textContent(),
          "Купленная редакция " + fixture.expected_version_no);
        assert.equal(await page.locator(".reader-topbar img[src=x]").count(), 0,
          "Untrusted book title must remain literal text");
        assert.equal(await page.locator(".reader-topbar__title script").count(), 0,
          "No HTML parsed from private book metadata");
        assert.equal(await page.locator(".reader-topbar__book").getAttribute("title"),
          fixture.expected_book_title);
        // The native <details> menu is server-rendered from the buyer's D
        // entitlements; it must link to the second verified paid edition.
        const toc = page.locator("[data-paid-toc]");
        assert.equal(await toc.count(), 1, device + ": purchased TOC missing");
        await toc.locator("summary").click();
        assert(await toc.evaluate((element) => element.open),
          device + ": purchased TOC does not open");
        const second = toc.locator("a[data-paid-toc-link]");
        assert.equal(await second.count(), 1, device + ": expected one other paid edition");
        assert.equal(await second.getAttribute("href"),
          "/read/paid/" + fixture.toc_second_chapter_id +
          "?purchase_id=" + fixture.toc_second_purchase_id);
        assert((await second.textContent()).includes(fixture.toc_second_title),
          device + ": TOC must display actual PostgreSQL chapter title");
        assert.equal(await toc.locator("script, img[src=x]").count(), 0,
          device + ": purchased TOC parsed user HTML");
        assert.equal(await toc.locator('[aria-current="page"]').count(), 1,
          device + ": current pinned edition missing from TOC");
        const tocOverflow = await page.evaluate(() =>
          document.documentElement.scrollWidth >
          document.documentElement.clientWidth + 1);
        assert(!tocOverflow, device + ": TOC opens outside the viewport");
        await toc.locator("summary").click();
        assert(!(await toc.evaluate((element) => element.open)),
          device + ": purchased TOC did not close");

        const state = await page.evaluate(async () => ({
          overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
          xss: window.__xss === true,
          local: localStorage.length + sessionStorage.length,
          caches: (await caches.keys()).length,
          indexed: (await indexedDB.databases()).length,
        }));
        assert(!state.overflow && !state.xss && !state.local &&
          !state.caches && !state.indexed, device + ": unsafe reader state");

        for (const resource of ["/assets/site.css", "/assets/newboo.css"]) {
          assert(loadedAssets.has(resource), device + ": missing reader stylesheet " + resource);
        }
        assert.equal(await page.locator(".site-header, .site-footer, .site-logo").count(), 0,
          device + ": unrelated site navigation exposed inside paid reader");
        assert.equal(await page.locator("main#main").count(), 1,
          device + ": reader document has no main landmark");
        assert.equal(await page.locator(".reader-topbar__back").getAttribute("href"),
          "/me/purchases", device + ": buyer has no safe way out");

        const metrics = await page.evaluate(() => {
          const readerWidth = document.querySelector(".reader-paid .reader-width");
          const content = document.querySelector(".reader-paid__content");
          const body = getComputedStyle(document.body);
          const textStyle = getComputedStyle(document.querySelector(".reader-piece__text"));
          const contentStyle = getComputedStyle(content);
          const topbarStyle = getComputedStyle(document.querySelector(".reader-topbar"));
          return {
            bodyMargin: body.marginTop,
            background: body.backgroundColor,
            readerWidth: readerWidth.getBoundingClientRect().width,
            maxWidth: Number.parseFloat(getComputedStyle(readerWidth).maxWidth),
            font: textStyle.fontFamily,
            lineHeight: Number.parseFloat(textStyle.lineHeight),
            padding: Number.parseFloat(contentStyle.paddingLeft),
            border: contentStyle.borderTopStyle,
            topbarPosition: topbarStyle.position,
            topbarTop: Number.parseFloat(topbarStyle.top),
            stylesheets: document.styleSheets.length,
          };
        });
        assert.equal(metrics.bodyMargin, "0px", device + ": site.css not applied");
        assert.equal(await page.locator(".reader-paid .reader-topbar").evaluate((element) =>
          getComputedStyle(element).display), "grid", device + ": original reader topbar lost");
        assert(metrics.readerWidth <= 860 && metrics.readerWidth >= 300,
          device + ": original NEwBOO reader width not respected");
        assert(metrics.font.includes("Georgia") && metrics.lineHeight > 24,
          device + ": reader typography lost");
        assert(metrics.padding >= 12 && metrics.border === "solid",
          device + ": paid reading surface lost its padding/border");
        assert(metrics.topbarPosition === "sticky" && metrics.topbarTop === 0,
          device + ": standalone NEwBOO reader topbar not sticky");
        assert(metrics.stylesheets >= 2, device + ": layout stylesheets missing");

        // Render the real Rust HTML + exact production CSS on two viewports.
        // Mask decrypted synthetic text so CI artifacts never contain paid content.
        const mask = [page.locator("[data-paid-content] .reader-piece__text")];
        await page.screenshot({
          path: path.join(screenshotRoot, "s034-" + device + "-light.png"),
          fullPage: true, mask, maskColor: "#e5e7eb",
        });
        await page.emulateMedia({ colorScheme: "dark" });
        const darkBackground = await page.evaluate(() =>
          getComputedStyle(document.body).backgroundColor);
        assert.notEqual(darkBackground, metrics.background,
          device + ": real NEwBOO dark tokens not applied");
        await page.screenshot({
          path: path.join(screenshotRoot, "s034-" + device + "-dark.png"),
          fullPage: true, mask, maskColor: "#e5e7eb",
        });
        // Controls must be real interactions, not decorative buttons. No
        // browser storage is used for paid text, keys or preferences.
        const panel = page.locator("#paid-reader-settings");
        const control = page.getByRole("button", { name: "Настройки чтения" });
        assert(await panel.isHidden(), device + ": reader settings shown by default");
        await control.click();
        assert(await panel.isVisible(), device + ": settings button does not open panel");
        assert.equal(await control.getAttribute("aria-expanded"), "true");
        const sizeBefore = await page.locator(".reader-piece__text").first().evaluate(
          (element) => Number.parseFloat(getComputedStyle(element).fontSize));
        await page.locator("[data-paid-font-scale]").evaluate((input) => {
          input.value = "120";
          input.dispatchEvent(new Event("input", { bubbles: true }));
        });
        const sizeAfter = await page.locator(".reader-piece__text").first().evaluate(
          (element) => Number.parseFloat(getComputedStyle(element).fontSize));
        assert(sizeAfter > sizeBefore, device + ": font size setting ignored");
        assert.equal(await page.locator("[data-paid-font-output]").textContent(), "120%");
        await page.locator("[data-paid-line-height]").evaluate((input) => {
          input.value = "205";
          input.dispatchEvent(new Event("input", { bubbles: true }));
        });
        const spacing = await page.locator(".reader-piece__text").first().evaluate(
          (element) => Number.parseFloat(getComputedStyle(element).lineHeight));
        assert(spacing > sizeAfter * 2, device + ": line spacing setting ignored");
        await page.locator("[data-paid-reader-width]").evaluate((input) => {
          input.value = "620";
          input.dispatchEvent(new Event("input", { bubbles: true }));
        });
        if (device === "desktop") {
          const newWidth = await page.locator(".reader-paid .reader-width").evaluate(
            (element) => element.getBoundingClientRect().width);
          assert(newWidth <= 620 && newWidth < metrics.readerWidth,
            "reader width setting ignored");
        }
        await page.getByRole("button", { name: "Сепия" }).click();
        const sepiaBackground = await page.evaluate(() =>
          getComputedStyle(document.body).backgroundColor);
        assert.notEqual(sepiaBackground, darkBackground, device + ": sepia theme ignored");
        assert.equal(await page.getByRole("button", { name: "Сепия" })
          .getAttribute("aria-pressed"), "true");
        if (device === "desktop") {
          await page.screenshot({
            path: path.join(screenshotRoot, "s065-reader-settings-desktop.png"),
            fullPage: true, mask, maskColor: "#e5e7eb",
          });
        }
        await page.getByRole("button", { name: "Сбросить настройки" }).click();
        assert.equal(await page.locator("[data-paid-font-output]").textContent(), "100%");
        assert.equal(await page.evaluate(() => document.body.hasAttribute(
          "data-paid-reader-theme")), false, device + ": reset did not clear theme");
        await page.getByRole("button", { name: "Закрыть настройки" }).click();
        assert(await panel.isHidden(), device + ": settings close did not work");
        assert.equal(await control.getAttribute("aria-expanded"), "false");
        await control.click();
        await page.keyboard.press("Escape");
        assert(await panel.isHidden(), device + ": Escape must close reader controls");
        assert(await control.evaluate((element) => element === document.activeElement),
          device + ": Escape did not return keyboard focus");
        const storageCount = await page.evaluate(async () =>
          localStorage.length + sessionStorage.length + (await caches.keys()).length +
          (await indexedDB.databases()).length);
        assert.equal(storageCount, 0, device + ": settings wrote into persistent storage");

        await page.getByRole("button", { name: "Сохранить после фрагмента 1" }).click();
        await page.getByText("Место сохранено.").waitFor();
        const first = fixture.expected_markdown.split("\n\n")[0];
        assert.deepEqual(saved, {
          chapter_id: chapter, purchase_id: purchase, version_id: grant.version_id,
          block_id: 1, char_offset: first.length, expected_revision: 0,
        });
        const section = page.locator("[data-paid-completion]");
        assert(await section.isVisible(), device + ": missing completion UI");
        if (device === "desktop") {
          await page.getByRole("button", { name: "Глава прочитана" }).click();
          await section.getByText("Глава отмечена прочитанной.", { exact: false })
            .waitFor({ timeout: 10000 });
          assert.equal(completionPosts, 1, "only explicit click completes a chapter");
          assert.equal(await section.locator('a[href="/me/rankings"]').count(), 1);
        } else {
          await section.getByText("Глава уже отмечена прочитанной.").waitFor();
          assert.equal(await page.getByRole("button", { name: "Глава прочитана" }).count(), 0);
          assert.equal(completionPosts, 1, "reopening does not duplicate completion");
        }
      } finally {
        await page.close();
      }
    }
    assert.equal(grantCalls, 2, "No unexpected or repeated paid grants");
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }

  const remaining = signatureExpiry(short) + 2500 - Date.now();
  if (remaining > 0) await new Promise((resolve) => setTimeout(resolve, remaining));
  const expired = await fetch(short);
  assert.equal(expired.status, 403, "Expired S3 URL must be rejected");
  console.log("H/S034 Chromium + real private MinIO CORS PASS; 30s signed URL expired");
}

run().catch((error) => {
  // Only sanitized assertion messages, never URLs, tokens or keys.
  console.error("H/S034 real Chromium/MinIO failed:", error.message);
  server.close();
  process.exitCode = 1;
});
