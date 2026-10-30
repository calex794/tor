// D-05/H: Chromium runs the actual Layer browser module against a real
// PG-authorized Rust shell and a real private MinIO ciphertext grant.
// All bearer URLs/DEKs stay in the disposable runner fixture and process.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3098";
const fixturePath = process.env.TOR_PAID_GRANT_FIXTURE_PATH;
assert(fixturePath && process.env.RUNNER_TEMP &&
  path.resolve(fixturePath).startsWith(path.resolve(process.env.RUNNER_TEMP) + path.sep),
"Reader fixture must remain inside the disposable CI directory");
const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
const html = fixture.layer_reader_html;
const grant = fixture.grant;
const chapter = grant.chapter_id;
assert(typeof html === "string" && html.includes("data-layer-chapter-reader") &&
  html.includes('data-chapter-id="' + chapter + '"'));
assert(!html.includes(fixture.expected_markdown) &&
  !html.includes(grant.content.dek_b64u) && !html.includes("data-purchase-id"));
const csrf = html.match(/data-csrf-token="([A-Za-z0-9_-]{43})"/)?.[1];
assert(csrf, "Layer shell must provide server-generated CSRF token");

const scripts = {
  "/assets/newboo-paid-reader.mjs":
    fs.readFileSync("crates/web/assets/newboo-paid-reader.mjs"),
  "/assets/newboo-layer-reader.mjs":
    fs.readFileSync("crates/web/assets/newboo-layer-reader.mjs"),
};
const siteCss = fs.readFileSync("crates/web/assets/site.css");
const newbooCss = [
  "newboo-port.css", "newboo-home-carousel.css", "newboo-book-detail.css",
  "newboo-reader.css", "newboo-cover-picker.css",
].map(name => fs.readFileSync("crates/web/assets/" + name)).join("\n");

function send(reply, code, body, type, extra = {}) {
  reply.writeHead(code, {
    "content-type": type,
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "referrer-policy": "no-referrer",
    ...extra,
  });
  reply.end(body);
}
function receive(req) {
  return new Promise((resolve, reject) => {
    let raw = "";
    req.on("data", chunk => {
      raw += chunk.toString("utf8");
      if (raw.length > 512) {
        req.destroy();
        reject(new Error("Unexpected request size"));
      }
    });
    req.on("end", () => resolve(raw));
    req.on("error", reject);
  });
}
let access = true;
let grantCalls = 0;
let openCalls = 0;
let unexpectedProgress = 0;
const server = http.createServer(async (req, reply) => {
  try {
    const url = new URL(req.url, SITE);
    if (req.method === "GET" && url.pathname === "/read/layer/" + chapter) {
      send(reply, 200, html, "text/html; charset=utf-8", {
        "content-security-policy": fixture.reader_csp,
      });
    } else if (req.method === "GET" && scripts[url.pathname]) {
      send(reply, 200, scripts[url.pathname], "application/javascript; charset=utf-8");
    } else if (req.method === "GET" && url.pathname === "/assets/site.css") {
      send(reply, 200, siteCss, "text/css; charset=utf-8");
    } else if (req.method === "GET" && url.pathname === "/assets/newboo.css") {
      send(reply, 200, newbooCss, "text/css; charset=utf-8");
    } else if (req.method === "POST" &&
      url.pathname === "/api/v1/reader/chapters/" + chapter + "/delivery-grants") {
      assert.equal(req.headers.origin, SITE);
      assert.equal(req.headers["x-csrf-token"], csrf);
      assert.equal(req.headers["content-type"], "application/json");
      assert.deepEqual(JSON.parse(await receive(req)), { source: "layer" });
      grantCalls++;
      if (access) send(reply, 200, JSON.stringify(grant), "application/json");
      else send(reply, 403, "", "text/plain");
    } else if (req.method === "POST" &&
      url.pathname === "/api/v1/reader/chapters/" + chapter + "/open-confirmations") {
      assert.equal(req.headers.origin, SITE);
      assert.equal(req.headers["x-csrf-token"], csrf);
      assert.deepEqual(JSON.parse(await receive(req)), {
        source: "layer", version_id: grant.version_id,
      });
      assert(access && grantCalls >= 1, "only a successful Layer reader may ACK");
      openCalls++;
      send(reply, 200, JSON.stringify({ status: "recorded" }), "application/json");
    } else if (url.pathname.includes("/progress/purchased")) {
      unexpectedProgress++;
      send(reply, 403, "", "text/plain");
    } else {
      send(reply, 404, "", "text/plain");
    }
  } catch {
    // Never log a cookie, signed URL, key or chapter plaintext.
    send(reply, 503, "", "text/plain");
  }
});

async function run() {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(3098, "127.0.0.1", resolve);
  });
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    const response = await page.goto(SITE + "/read/layer/" + chapter);
    assert.equal(response.status(), 200);
    await page.locator("[data-layer-status]").filter({
      hasText: "Глава открыта",
    }).waitFor({ timeout: 20000 });
    const visible = await page.locator("[data-layer-content]").textContent();
    assert(visible.includes(fixture.expected_markdown.split("\n\n")[0]));
    assert.equal(await page.locator("[data-purchase-id]").count(), 0);
    assert.equal(await page.locator("[data-paid-completion]").count(), 0);
    assert.equal(grantCalls, 1);
    assert.equal(openCalls, 1, "a rendered Layer chapter must acknowledge once");
    await page.close();

    // Simulate revocation after the HTML shell was received but before POST.
    access = false;
    const revoked = await browser.newPage();
    await revoked.goto(SITE + "/read/layer/" + chapter);
    await revoked.locator("[data-layer-status]").filter({
      hasText: "Доступ к главе недоступен",
    }).waitFor({ timeout: 10000 });
    assert.equal(await revoked.locator("[data-layer-content]").textContent(), "");
    assert.equal(grantCalls, 2);
    assert.equal(openCalls, 1, "revoked grant must not create an opening ACK");
    assert.equal(unexpectedProgress, 0,
      "Layer must not fabricate a direct purchase to save progress");
    await revoked.close();
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
}

run().catch(() => {
  // CI reports failure without printing grants, cookies or plaintext.
  process.exitCode = 1;
  server.close();
});
