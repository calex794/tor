// S065: the site administrator's private reader. No purchase_id, direct
// wallet progress, key, signed URL or decrypted chapter is persisted.
import {
  decryptPaidGrant, decryptPaidAsset, renderPaidBlocks,
} from "/assets/newboo-paid-reader.mjs";

function requireValue(value) {
  if (!value) throw new Error("Административное чтение недоступно.");
}

async function adminGrant(chapter, csrf, imageId = null) {
  const path = "/api/v1/reader/chapters/" + chapter +
    (imageId ? "/assets/" + imageId : "") + "/delivery-grants";
  const response = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
    credentials: "same-origin",
    redirect: "error",
    cache: "no-store",
    referrerPolicy: "no-referrer",
    body: JSON.stringify({ source: "administrator" }),
  });
  requireValue(response.ok && response.status === 200);
  return response.json();
}

export async function startAdministratorReader(root) {
  const chapter = root.dataset.chapterId;
  const csrf = root.dataset.csrfToken;
  const s3Origin = root.dataset.s3Origin;
  const status = root.querySelector("[data-admin-status]");
  const article = root.querySelector("[data-admin-content]");
  requireValue(
    /^[1-9][0-9]{0,18}$/.test(chapter || "") &&
    /^[A-Za-z0-9_-]{43}$/.test(csrf || "") &&
    typeof s3Origin === "string" && s3Origin.length > 0 &&
    status && article
  );
  const grant = await adminGrant(chapter, csrf);
  const { versionId, text } = await decryptPaidGrant(grant, chapter, s3Origin);
  const liveUrls = new Map();
  let closed = false;
  let active = 0;
  let observer = null;
  const tasks = [];
  const known = new Map();

  const clear = () => {
    closed = true;
    observer?.disconnect();
    for (const url of liveUrls.values()) URL.revokeObjectURL(url);
    liveUrls.clear();
    tasks.length = 0;
    known.clear();
    article.replaceChildren();
    status.textContent = "Административная глава закрыта.";
  };
  window.addEventListener("pagehide", clear, { once: true });
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) location.reload();
  });

  async function show({ frame, id, alt }) {
    if (closed || !frame.isConnected) return;
    try {
      const asset = await adminGrant(chapter, csrf, id);
      const blob = await decryptPaidAsset(
        asset, chapter, versionId, id, s3Origin
      );
      if (closed || !frame.isConnected) return;
      while (liveUrls.size >= 12) {
        const [oldFrame, oldUrl] = liveUrls.entries().next().value;
        liveUrls.delete(oldFrame);
        URL.revokeObjectURL(oldUrl);
        oldFrame.textContent = "Изображение выгружено из памяти.";
      }
      const url = URL.createObjectURL(blob);
      liveUrls.set(frame, url);
      const image = document.createElement("img");
      image.className = "reader-content__image";
      image.alt = alt;
      image.loading = "lazy";
      image.referrerPolicy = "no-referrer";
      image.addEventListener("load", () => frame.classList.add("is-loaded"), { once: true });
      image.addEventListener("error", () => {
        URL.revokeObjectURL(url);
        if (liveUrls.get(frame) === url) liveUrls.delete(frame);
        frame.textContent = "Не удалось открыть иллюстрацию.";
      }, { once: true });
      image.src = url;
      frame.replaceChildren(image);
    } catch {
      if (!closed && frame.isConnected) frame.textContent = "Иллюстрация недоступна.";
    }
  }
  function drain() {
    while (!closed && active < 2 && tasks.length) {
      const task = tasks.shift();
      if (!task.frame.isConnected) continue;
      active++;
      show(task).finally(() => {
        active--;
        drain();
      });
    }
  }
  function schedule(frame) {
    const task = known.get(frame);
    if (!task || closed) return;
    known.delete(frame);
    tasks.push(task);
    drain();
  }
  if (typeof IntersectionObserver === "function") {
    observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        observer.unobserve(entry.target);
        schedule(entry.target);
      }
    }, { rootMargin: "300px" });
  }

  // Reuse the exact private-image marker parser. Remove the direct reader's
  // "save place" buttons: admin inspection is not a purchased reading fact.
  renderPaidBlocks(article, text, () => {}, (frame, id, alt) => {
    requireValue(known.size < 64);
    known.set(frame, { frame, id, alt });
    if (observer) observer.observe(frame);
    else schedule(frame);
  });
  for (const button of article.querySelectorAll(".reader-save-button")) {
    button.remove();
  }
  status.textContent = "Редакция " + versionId + " · Защищённая загрузка завершена";
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-admin-chapter-reader]");
  if (root) {
    startAdministratorReader(root).catch(() => {
      root.querySelector("[data-admin-content]")?.replaceChildren();
      const status = root.querySelector("[data-admin-status]");
      if (status) status.textContent = "Административный доступ недоступен.";
    });
  }
}
