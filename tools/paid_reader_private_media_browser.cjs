// H/S065: actual Chromium decrypts and renders a private S3 image only after
// receiving an exact D/B/G asset grant. The S3 endpoint is a ciphertext mock;
// the existing paid-grant suite separately covers real PostgreSQL and MinIO.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { chromium } = require("playwright");
const { createCipheriv, createHash } = require("node:crypto");
const { gzipSync, deflateSync } = require("node:zlib");

const SITE = "http://127.0.0.1:3098";
const S3 = "http://127.0.0.1:9000";
const CHAPTER = "103", VERSION = "201", PURCHASE = "501";
const TEXT_ID = "a01e4711-2119-48a1-b280-74b38ac1b9a2";
const IMAGE_ID = "b01e4711-2119-48a1-b280-74b38ac1b9a2";
const CSRF = "c".repeat(43);
const PAYLOAD = "Текст перед иллюстрацией\n\n" +
  "![Рисунок платной главы](asset:" + IMAGE_ID + ")\n\n" +
  "<img src=x onerror=window.__xss=true>";

const sha = (b) => createHash("sha256").update(b).digest("hex");
function crc32(bytes) {
  let crc = -1;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i++)
      crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ -1) >>> 0;
}
function pngChunk(name, data) {
  const type = Buffer.from(name, "ascii");
  const size = Buffer.alloc(4);
  size.writeUInt32BE(data.length);
  const content = Buffer.concat([type, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(content));
  return Buffer.concat([size, content, crc]);
}
function validPng() {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(1, 0);
  ihdr.writeUInt32BE(1, 4);
  ihdr[8] = 8;  // RGBA, 8-bit
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from("89504e470d0a1a0a", "hex"),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(Buffer.from([0, 255, 0, 0, 255]))),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}
function envelope(objectId, kind, plaintext) {
  const bytes = Buffer.from(plaintext);
  const dek = Buffer.alloc(32, kind === "chapter" ? 31 : 47);
  const nonce = Buffer.alloc(12, kind === "chapter" ? 5 : 9);
  const metadata = kind === "chapter" ? "tor.encrypted.chapter.v1" : "tor.encrypted.asset.v1";
  const aad = Buffer.from(metadata + "\n" + CHAPTER + "\n" + VERSION +
    "\n" + objectId + "\n" + sha(bytes) + "\n");
  const cipher = createCipheriv("aes-256-gcm", dek, nonce);
  cipher.setAAD(aad);
  const body = kind === "chapter" ? gzipSync(bytes) : bytes;
  const ciphertext = Buffer.concat([
    cipher.update(body), cipher.final(), cipher.getAuthTag(),
  ]);
  return {
    ciphertext,
    grantContent: {
      ciphertext_url: S3 + "/tor-private-chapter-ci/private/" +
        (kind === "chapter" ? "chapter-versions/" : "chapter-assets/") +
        objectId + ".enc?X-Amz-Signature=" + "a".repeat(64),
      object_sha256: sha(ciphertext),
      cipher: "AES-256-GCM",
      codec: kind === "chapter" ? "gzip" : "none",
      nonce_b64u: nonce.toString("base64url"),
      aad_b64u: aad.toString("base64url"),
      dek_b64u: dek.toString("base64url"),
    },
  };
}
const script = fs.readFileSync("crates/web/assets/newboo-paid-reader.mjs", "utf8");
const pageMarkup = '<!doctype html><html lang="ru"><head>' +
  '<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">' +
  '</head><body><section data-paid-chapter-reader ' +
  'data-chapter-id="' + CHAPTER + '" data-purchase-id="' + PURCHASE + '" ' +
  'data-csrf-token="' + CSRF + '" data-s3-origin="' + S3 + '">' +
  '<p data-paid-status role="status"></p><div data-paid-resume></div>' +
  '<div data-paid-completion hidden></div>' +
  '<article data-paid-content aria-label="Текст главы"></article></section>' +
  '<script type="module" src="/assets/newboo-paid-reader.mjs"></script></body></html>';

async function caseInBrowser(browser, { mode, viewport }) {
  const page = await browser.newPage({ viewport });
  const body = envelope(TEXT_ID, "chapter", PAYLOAD);
  const png = validPng();
  const image = envelope(IMAGE_ID, "asset", mode === "gif" ?
    Buffer.from("47494638396101000100800000ffffff00000021f90401000000002c00000000010001000002024401003b", "hex") : png);
  const chapterGrant = {
    schema: "tor.paid_delivery.v1",
    chapter_id: CHAPTER, version_id: VERSION,
    expires_at: new Date(Date.now() + 90000).toISOString(),
    protected_mode: "pua_v1", content: body.grantContent, assets: [],
  };
  const assetGrant = {
    schema: "tor.paid_asset_delivery.v1",
    chapter_id: CHAPTER, version_id: VERSION, object_id: IMAGE_ID,
    expires_at: new Date(Date.now() + 90000).toISOString(),
    content: { ...image.grantContent },
  };
  if (mode === "tampered") assetGrant.content.object_sha256 = "0".repeat(64);
  if (mode === "other-edition") assetGrant.version_id = "202";
  if (mode === "foreign-origin") {
    assetGrant.content.ciphertext_url =
      "https://hostile.invalid/private/chapter-assets/" + IMAGE_ID +
      ".enc?X-Amz-Signature=attack";
  }
  let s3Chapter = 0, s3Image = 0, assetGrants = 0;
  await page.route("**/read/paid/103?purchase_id=501", (route) => route.fulfill({
    status: 200,
    contentType: "text/html; charset=utf-8",
    headers: {
      "cache-control": "private, no-store",
      "content-security-policy": "default-src 'none'; script-src 'self'; " +
        "style-src 'self'; img-src 'self' blob:; connect-src 'self' " + S3 +
        "; base-uri 'none'; frame-ancestors 'none'",
    },
    body: pageMarkup,
  }));
  await page.route("**/assets/newboo-paid-reader.mjs", route => route.fulfill({
    status: 200, contentType: "application/javascript", body: script,
  }));
  await page.route("**/api/v1/reader/chapters/103/delivery-grants", route => {
    assert.equal(route.request().method(), "POST");
    assert.equal(route.request().headers()["x-csrf-token"], CSRF);
    assert.deepEqual(route.request().postDataJSON(), { purchase_id: PURCHASE });
    return route.fulfill({
      status: 200, contentType: "application/json",
      headers: { "cache-control": "private, no-store" },
      body: JSON.stringify(chapterGrant),
    });
  });
  await page.route("**/api/v1/reader/chapters/103/assets/*/delivery-grants", route => {
    assetGrants++;
    assert.equal(route.request().method(), "POST");
    assert.equal(route.request().headers()["x-csrf-token"], CSRF);
    assert.equal(route.request().url().includes("/" + IMAGE_ID + "/"), true);
    assert.deepEqual(route.request().postDataJSON(), { purchase_id: PURCHASE });
    return route.fulfill({
      status: 200, contentType: "application/json",
      headers: { "cache-control": "private, no-store" },
      body: JSON.stringify(assetGrant),
    });
  });
  await page.route(S3 + "/**", route => {
    const req = route.request();
    assert.equal(req.headers().referer, undefined, "signed S3 URL leaked a Referer");
    assert.equal(req.method(), "GET");
    let data;
    if (req.url().includes("/chapter-versions/")) {
      data = body.ciphertext;
      s3Chapter++;
    } else {
      assert(req.url().includes("/private/chapter-assets/" + IMAGE_ID + ".enc"));
      data = image.ciphertext;
      s3Image++;
    }
    return route.fulfill({
      status: 200,
      contentType: "application/octet-stream",
      headers: { "access-control-allow-origin": SITE,
        "content-length": String(data.length) },
      body: data,
    });
  });
  await page.route("**/api/v1/me/progress/purchased**", route => {
    const u = new URL(route.request().url());
    const result = u.pathname.endsWith("/complete")
      ? { available: false, completed: false }
      : { revision: 0, version_id: VERSION, block_id: null };
    return route.fulfill({
      status: 200, contentType: "application/json", body: JSON.stringify(result),
    });
  });
  await page.addInitScript(() => {
    window.__revokedBlobs = 0;
    const original = URL.revokeObjectURL.bind(URL);
    URL.revokeObjectURL = (u) => {
      window.__revokedBlobs++;
      return original(u);
    };
  });

  try {
    await page.goto(SITE + "/read/paid/103?purchase_id=501", { waitUntil: "networkidle" });
    await page.getByText("Защищённая загрузка завершена", { exact: false }).waitFor();
    const frame = page.locator(".reader-paid__image-block .reader-content__image-frame");
    assert.equal(await frame.count(), 1, "one private figure should replace exact image marker");
    assert.equal((await page.locator(".reader-paid__image-block figcaption").textContent()),
      "Рисунок платной главы");
    if (mode === "valid") {
      await page.waitForFunction(() => {
        const img = document.querySelector(".reader-paid__image-block img");
        return img && img.complete && img.naturalWidth === 1 && img.naturalHeight === 1;
      });
      const rendered = await page.locator(".reader-paid__image-block img").evaluate(el => ({
        src: el.getAttribute("src"), alt: el.alt, referrerPolicy: el.referrerPolicy,
      }));
      assert(rendered.src.startsWith("blob:"), "only in-memory image URLs are allowed");
      assert.equal(rendered.alt, "Рисунок платной главы");
      assert.equal(rendered.referrerPolicy, "no-referrer");
      assert.equal(s3Image, 1, "exactly one private encrypted asset GET");
      assert.equal(assetGrants, 1, "exactly one authorized D/B/G media grant");
      const safe = await page.evaluate(() => {
        const article = document.querySelector("[data-paid-content]");
        const storageEmpty = localStorage.length === 0 && sessionStorage.length === 0;
        return { xss: window.__xss === true,
          unsafe: !!article.querySelector("img[src=x]"),
          hasSignedLink: article.innerHTML.includes("X-Amz-Signature"),
          storageEmpty, hasMedia: article.innerHTML.includes("asset:"), };
      });
      assert.deepEqual(safe, {
        xss: false, unsafe: false, hasSignedLink: false,
        storageEmpty: true, hasMedia: false,
      });
      await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
      assert((await page.evaluate(() => window.__revokedBlobs)) >= 1,
        "paid blob URLs must be revoked on pagehide");
    } else {
      await frame.getByText("Иллюстрация недоступна", { exact: false }).waitFor();
      assert.equal(await frame.locator("img").count(), 0);
      assert.equal(await page.locator(".reader-piece__text").first().textContent(),
        "Текст перед иллюстрацией", "text must remain when an image grant fails");
      assert.equal(s3Image, mode === "tampered" || mode === "gif" ? 1 : 0,
        "invalid version or origin should fail before S3");
    }
    assert.equal(s3Chapter, 1, "chapter ciphertext always came from S3 mock");
    const width = await page.evaluate(() => [
      document.documentElement.scrollWidth, document.documentElement.clientWidth,
    ]);
    assert(width[0] <= width[1] + 1, "private image overflowed viewport");
  } finally {
    await page.close();
  }
}

(async () => {
  const browser = await chromium.launch({ headless: true, args: ["--no-sandbox"] });
  try {
    for (const mode of ["valid", "tampered", "other-edition", "foreign-origin", "gif"]) {
      await caseInBrowser(browser, { mode, viewport: { width: 390, height: 844 } });
    }
    await caseInBrowser(browser, { mode: "valid", viewport: { width: 1440, height: 900 } });
    console.log("H/S065 private paid media: Chromium/mobile, GCM, PNG, tamper, origin, MIME, blob cleanup PASS");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
