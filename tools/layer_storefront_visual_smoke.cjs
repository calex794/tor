// D-05 / S031: genuine PostgreSQL-backed Rust page renderings in Chromium.
// Only screenshots are archived; ephemeral fixtures and wallet/CSRF tokens are not.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const http = require("node:http");
const { chromium } = require("playwright");

const SITE = "http://127.0.0.1:3109";
const ciTemp = path.resolve(process.env.RUNNER_TEMP || "");
const fixturePath = path.resolve(process.env.TOR_LAYER_VISUAL_FIXTURE_PATH || "");
const shots = path.resolve(process.env.TOR_LAYER_SHOTS || "");
assert(process.env.RUNNER_TEMP && fixturePath.startsWith(ciTemp + path.sep),
  "Layer fixture must be an ephemeral CI-only file");
assert(process.env.TOR_LAYER_SHOTS && shots.startsWith(ciTemp + path.sep),
  "Screenshots must be under the disposable CI temp directory");
const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
for (const name of ["guest", "buyer", "library", "purchased"]) {
  assert(typeof fixture[name] === "string" && fixture[name].includes('<html lang="ru">'),
    "Must use actual Rust server-rendered page: " + name);
}
assert(!fixture.guest.includes("SECRET-DRAFT-LAYER"));
assert(!fixture.buyer.includes("SECRET-DRAFT-LAYER"));
assert(!fixture.library.includes("SECRET-DRAFT-LAYER"));
assert(!fixture.library.includes("SECRET-LAYER-CHAPTER-PLAIN-TEXT"));

const expectedStyles = [
  "newboo-port.css", "newboo-home-carousel.css",
  "newboo-book-detail.css", "newboo-comments.css", "newboo-reader.css",
  "newboo-cover-picker.css", "newboo-layer-storefront.css",
];
const source = fs.readFileSync("crates/web/src/pages.rs", "utf8");
const start = source.indexOf("pub async fn newboo_stylesheet()");
const end = source.indexOf("pub async fn paid_reader_script()", start);
assert(start !== -1 && end > start, "NEwBOO asset handler missing");
const included = [...source.slice(start, end).matchAll(
  /include_str!\("\.\.\/assets\/([^"]+\.css)"\)/g
)].map(match => match[1]);
assert.deepEqual(included, expectedStyles, "Browser must serve the exact Rust stylesheet set");
const siteCss = fs.readFileSync("crates/web/assets/site.css");
const newbooCss = expectedStyles.map(name =>
  fs.readFileSync(path.join("crates/web/assets", name), "utf8")).join("\n");
const logo = fs.readFileSync("crates/web/assets/brand/pandaolate-header.png");
const darkLogo = fs.readFileSync("crates/web/assets/brand/pandaolate-header-dark.png");

const htmlRoutes = {
  "/guest": fixture.guest,
  "/buyer": fixture.buyer,
  "/library": fixture.library,
  "/purchased": fixture.purchased,
};
const webServer = http.createServer((req, res) => {
  const pathname = new URL(req.url, SITE).pathname;
  const entry = htmlRoutes[pathname];
  let status = 200, contentType = "text/html; charset=utf-8", data = entry;
  if (pathname === "/assets/site.css") {
    contentType = "text/css; charset=utf-8";
    data = siteCss;
  } else if (pathname === "/assets/newboo.css") {
    contentType = "text/css; charset=utf-8";
    data = newbooCss;
  } else if (pathname === "/assets/brand/pandaolate-header.png") {
    contentType = "image/png"; data = logo;
  } else if (pathname === "/assets/brand/pandaolate-header-dark.png") {
    contentType = "image/png"; data = darkLogo;
  } else if (typeof entry !== "string") {
    status = 404; contentType = "text/plain; charset=utf-8"; data = "Not found";
  }
  res.writeHead(status, {
    "content-type": contentType,
    "cache-control": "private, no-store",
    "x-content-type-options": "nosniff",
  });
  res.end(data);
});

async function inspect(page, device, label) {
  const h1 = page.locator("h1").first();
  assert(await h1.isVisible(), device + "/" + label + ": missing heading");
  assert(await page.locator(".site-logo").isVisible(),
    device + "/" + label + ": NEwBOO logo missing");
  const result = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    scroll: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
    badScript: !!document.querySelector('script:not([src])'),
    dark: matchMedia("(prefers-color-scheme: dark)").matches,
  }));
  assert(result.scroll <= result.viewport + 1 &&
         result.body <= result.viewport + 1,
    device + "/" + label + ": horizontal overflow: " + JSON.stringify(result));
  assert(!result.badScript, device + "/" + label + ": untrusted inline script");
  assert(await page.getByText("SECRET-DRAFT-LAYER").count() === 0,
    device + "/" + label + ": hidden Book leaked");
}

(async () => {
  await new Promise(resolve => webServer.listen(3109, "127.0.0.1", resolve));
  fs.mkdirSync(shots, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [device, width, height] of [
      ["desktop", 1440, 900], ["mobile", 390, 844],
    ]) {
      for (const scheme of ["light", "dark"]) {
        const page = await browser.newPage({
          viewport: { width, height },
          colorScheme: scheme,
          deviceScaleFactor: 1,
        });
        try {
          page.on("dialog", dialog => { throw new Error("Unexpected script dialog: " + dialog.message()); });
          for (const [label, route] of [
            ["guest", "/guest"], ["buyer", "/buyer"], ["library", "/library"],
            ["purchased", "/purchased"],
          ]) {
            const response = await page.goto(SITE + route, { waitUntil: "networkidle" });
            assert(response && response.status() === 200, device + "/" + label + ": HTTP error");
            await inspect(page, device + "-" + scheme, label);
            if (label === "guest" || label === "buyer" || label === "purchased") {
              const books = page.locator(".layer-storefront-books .layer-storefront-book");
              assert.equal(await books.count(), 1, "Exactly one public Book");
              assert.equal(await page.locator(".layer-storefront-book").first().getAttribute("href"),
                "/books/" + fixture.book_id);
              assert(await page.locator(".layer-offer__price").isVisible(),
                "NEwBOO price block missing");
              assert((await page.locator(".layer-offer__price").innerText()).includes("1,25 ₽"),
                "Unexpected Layer price");
              const grid = page.locator(".layers-public-layout");
              const composition = page.locator(".layer-panel").first();
              const offer = page.locator(".layer-storefront-offer");
              const left = await composition.boundingBox();
              const right = await offer.boundingBox();
              assert(left && right && await grid.isVisible(), "Offer panels missing");
              if (width < 640) {
                assert(right.y >= left.y + left.height - 2, "Mobile offer must stack below books");
              } else {
                assert(right.x > left.x + left.width - 2, "Desktop offer must be a side column");
              }
              if (label === "guest") {
                assert(await page.getByRole("link", {
                  name: "Войти и приобрести доступ",
                }).isVisible(), "Guest sign-in missing");
                assert.equal(await page.locator(".layer-purchase-form").count(), 0);
              }
              if (label === "buyer") {
                const form = page.locator(".layer-purchase-form");
                assert(await form.isVisible(), "Native checkout form missing");
                assert.equal(await form.locator('input[name="expected_price_minor"]').inputValue(), "125");
                const confirm = form.locator('input[name="confirm"]');
                assert.equal(await confirm.getAttribute("required"), "");
                assert.equal(await form.evaluate(el => el.checkValidity()), false,
                  "Browser must refuse unchecked purchase");
                await confirm.check();
                assert.equal(await form.evaluate(el => el.checkValidity()), true,
                  "Confirmed form should be valid");
              }
              if (label === "purchased") {
                assert(await page.getByText("Доступ к пакету сейчас действует").isVisible(),
                  "Confirmed entitlement banner missing");
              }
            } else {
              assert(await page.getByRole("heading", {
                name: "Мои пакеты доступа",
              }).isVisible(), "Private purchased terms page missing");
              assert(await page.getByText("Доступен", { exact: true }).isVisible(),
                "Active paid term missing");
              assert.equal(await page.locator('a[href^="/read/layer/"]').count(), 0,
                "No fake paid reader link when S3 reader is not mounted");
            }
            await page.screenshot({
              path: path.join(shots, "layer-" + label + "-" + device + "-" + scheme + ".png"),
              fullPage: true,
            });
          }
        } finally {
          await page.close();
        }
      }
    }
    console.log("D-05 Layer NEwBOO Chromium PASS: 16 genuine PG/Rust screenshots, desktop/mobile, light/dark, consent and published-only books");
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error.stack || error);
  process.exitCode = 1;
}).finally(() => webServer.close());
