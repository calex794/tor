// H/S034: real Chromium WebCrypto/DOM/CSRF acceptance.
// UI mock isolates the browser; S034 Node suite separately covers real PG/MinIO.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { createCipheriv, createHash } = require("node:crypto");
const { gzipSync } = require("node:zlib");
const fs = require("node:fs");
const path = require("node:path");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const URL = SITE + "/read/paid/103?purchase_id=501";
const UUID = "a01e4711-2119-48a1-b280-74b38ac1b9a2";
const MARKDOWN = "Глава платная А😀Б\n\n<img src=x onerror=window.__xss=true>\n\n第二章";

function sealedFixture() {
  const dek = Buffer.alloc(32, 31);
  const nonce = Buffer.alloc(12, 5);
  const hash = createHash("sha256").update(MARKDOWN).digest("hex");
  const aad = Buffer.from(
    "tor.encrypted.chapter.v1\n103\n201\n" + UUID + "\n" + hash + "\n", "utf8"
  );
  const cipher = createCipheriv("aes-256-gcm", dek, nonce);
  cipher.setAAD(aad);
  const zipped = gzipSync(Buffer.from(MARKDOWN));
  const ciphertext = Buffer.concat([cipher.update(zipped), cipher.final(), cipher.getAuthTag()]);
  const objectSha = createHash("sha256").update(ciphertext).digest("hex");
  const signed = S3 + "/tor-private-chapter-ci/private/chapter-versions/" +
    UUID + ".enc?X-Amz-Signature=" + "a".repeat(64);
  return {
    ciphertext,
    grant: {
      schema: "tor.paid_delivery.v1",
      chapter_id: "103",
      version_id: "201",
      expires_at: new Date(Date.now() + 90000).toISOString(),
      protected_mode: "pua_v1",
      content: {
        ciphertext_url: signed, object_sha256: objectSha,
        cipher: "AES-256-GCM", codec: "gzip",
        nonce_b64u: nonce.toString("base64url"),
        aad_b64u: aad.toString("base64url"),
        dek_b64u: dek.toString("base64url"),
      },
      assets: [],
    },
  };
}

function html() {
  return '<!doctype html><html lang="ru"><head><meta charset="utf-8">' +
    '<meta name="viewport" content="width=device-width,initial-scale=1">' +
    '<link rel="stylesheet" href="/assets/newboo.css"><title>Купленная глава</title>' +
    '</head><body><main class="reader-page reader-private reader-paid" data-paid-chapter-reader ' +
    'data-chapter-id="103" data-purchase-id="501" data-csrf-token="' + "c".repeat(43) +
    '" data-s3-origin="' + S3 + '"><div class="container reader-width">' +
    '<header class="reader-topbar"><a class="reader-topbar__back" href="/me/library">' +
    '← Моя библиотека</a><span class="reader-topbar__chapter">Глава 103</span></header>' +
    '<h1>Глава 103</h1><p data-paid-status role="status"></p>' +
    '<div class="reader-resume" data-paid-resume></div>' +
    '<article class="chapter-content reader-markdown reader-paid__content" data-paid-content>' +
    '</article></div><script type="module" src="/assets/newboo-paid-reader.mjs"></script>' +
    '</main></body></html>';
}

async function inspect(page, device, success) {
  const report = await page.evaluate(() => ({
    width: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    xss: window.__xss === true,
    paragraphs: document.querySelectorAll(".reader-piece").length,
    cached: localStorage.length + sessionStorage.length,
  }));
  assert(report.width <= report.viewport + 1, device + ": horizontal overflow");
  assert(!report.xss, device + ": XSS executed");
  assert.equal(report.cached, 0, device + ": paid data stored in browser storage");
  assert.equal(report.paragraphs, success ? 3 : 0, device + ": wrong block count");
}

async function testDevice(browser, name, width, height, tamper) {
  const page = await browser.newPage({ viewport: { width, height } });
  const fixture = sealedFixture();
  const grant = JSON.parse(JSON.stringify(fixture.grant));
  if (tamper) grant.content.object_sha256 = "0".repeat(64);
  let grantCalls = 0, s3Calls = 0, saved = null;
  await page.route("**/read/paid/103?purchase_id=501", (route) =>
    route.fulfill({
      status: 200, contentType: "text/html; charset=utf-8", body: html(),
      headers: {
        "cache-control": "private, no-store",
        "referrer-policy": "no-referrer",
        "content-security-policy": "default-src 'none'; script-src 'self'; " +
          "style-src 'self'; connect-src 'self' " + S3 + "; base-uri 'none'",
      },
    }));
  await page.route("**/api/v1/reader/chapters/103/delivery-grants", async (route) => {
    grantCalls++;
    assert.equal(route.request().method(), "POST");
    assert.equal(route.request().headers()["x-csrf-token"], "c".repeat(43));
    assert.deepEqual(route.request().postDataJSON(), { purchase_id: "501" });
    await route.fulfill({
      status: 200, contentType: "application/json",
      headers: { "cache-control": "private, no-store" },
      body: JSON.stringify(grant),
    });
  });
  await page.route(S3 + "/**", async (route) => {
    s3Calls++;
    assert.equal(route.request().headers().referer, undefined, "S3 received referrer");
    await route.fulfill({
      status: 200, contentType: "application/octet-stream",
      headers: {
        "access-control-allow-origin": SITE,
        "content-length": String(fixture.ciphertext.length),
      },
      body: fixture.ciphertext,
    });
  });
  await page.route("**/api/v1/me/progress/purchased**", async (route) => {
    if (route.request().method() === "PUT") {
      saved = route.request().postDataJSON();
      await route.fulfill({ status: 200, contentType: "application/json",
        body: JSON.stringify({ revision: 1, version_id: "201" }) });
    } else {
      await route.fulfill({ status: 200, contentType: "application/json",
        body: JSON.stringify({ revision: 0, version_id: "201", block_id: null }) });
    }
  });
  try {
    const response = await page.goto(URL, { waitUntil: "networkidle" });
    assert.equal(response.status(), 200);
    const status = page.locator("[data-paid-status]");
    if (tamper) {
      await status.getByText("Не удалось открыть главу", { exact: false }).waitFor();
      await inspect(page, name, false);
    } else {
      await status.getByText("Защищённая загрузка завершена", { exact: false }).waitFor();
      assert.equal(await page.locator(".reader-piece__text").first().textContent(),
        "Глава платная А😀Б");
      assert((await page.locator(".reader-piece__text").nth(1).textContent())
        .includes("<img src=x onerror="), "HTML wasn't literal text");
      await inspect(page, name, true);
      await page.getByRole("button", { name: "Сохранить после фрагмента 1" }).click();
      await page.getByText("Место сохранено.").waitFor();
      assert.deepEqual(saved, {
        chapter_id: "103", purchase_id: "501", version_id: "201",
        block_id: 1, char_offset: "Глава платная А😀Б".length, expected_revision: 0,
      });
      assert.equal(await page.locator("img[src=x]").count(), 0, "Unsafe image parsed");
      if (process.env.TOR_PAID_READER_SHOTS) {
        fs.mkdirSync(process.env.TOR_PAID_READER_SHOTS, { recursive: true });
        await page.screenshot({
          path: path.join(process.env.TOR_PAID_READER_SHOTS, "paid-" + name + ".png"),
          fullPage: true,
        });
      }
    }
    assert.equal(grantCalls, 1, name + ": unexpected grant retries");
    assert.equal(s3Calls, 1, name + ": extra private S3 GET");
  } finally {
    await page.close();
  }
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    await testDevice(browser, "desktop", 1440, 900, false);
    await testDevice(browser, "mobile", 390, 844, false);
    await testDevice(browser, "tampered", 390, 844, true);
    console.log("H paid Chromium: WebCrypto, no XSS, UTF-16 cursor and tamper denial PASS");
  } finally {
    await browser.close();
  }
})().catch((e) => { console.error(e.message); process.exitCode = 1; });
