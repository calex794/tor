// D-05/H: Layer CURRENT edition, using the same verified ciphertext +
// browser WebCrypto implementation as pinned direct purchases. No purchase
// progress, entitlement cache, signed URL or DEK is persisted.
import { decryptPaidGrant, readerBlocks } from "/assets/newboo-paid-reader.mjs";

function requireValue(ok) {
  if (!ok) throw new Error("Не удалось безопасно открыть главу Layer.");
}

export async function startLayerReader(root) {
  const chapter = root.dataset.chapterId;
  const csrf = root.dataset.csrfToken;
  const s3Origin = root.dataset.s3Origin;
  const status = root.querySelector("[data-layer-status]");
  const article = root.querySelector("[data-layer-content]");
  requireValue(
    /^[1-9][0-9]{0,18}$/.test(chapter || "") &&
    /^[A-Za-z0-9_-]{43}$/.test(csrf || "") &&
    typeof s3Origin === "string" && s3Origin.length > 0 &&
    status && article
  );

  // The browser chooses only the entitlement SOURCE, not the edition,
  // version ID, storage object or private signing host.
  const reply = await fetch(
    "/api/v1/reader/chapters/" + chapter + "/delivery-grants",
    {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
      credentials: "same-origin",
      redirect: "error",
      cache: "no-store",
      referrerPolicy: "no-referrer",
      body: JSON.stringify({ source: "layer" }),
    }
  );
  requireValue(reply.status === 200 && reply.ok);
  const grant = await reply.json();
  const { versionId, text } = await decryptPaidGrant(grant, chapter, s3Origin);

  // Render text nodes only: never interpret decrypted Markdown as HTML.
  // Purchased progress/completion APIs require a purchase_id and MUST NOT
  // be reused for a Layer-only subscription.
  const sections = document.createDocumentFragment();
  readerBlocks(text).forEach((block, index) => {
    const section = document.createElement("section");
    section.className = "reader-piece";
    section.id = "layer-p-" + (index + 1);
    const paragraph = document.createElement("p");
    paragraph.className = "reader-piece__text reader-markdown";
    paragraph.textContent = block;
    section.append(paragraph);
    sections.append(section);
  });
  article.replaceChildren(sections);
  // The grant was issued earlier. Only after verified S3/crypto and DOM
  // rendering does this browser report a chapter opening.
  let openConfirmed = false;
  try {
    const opened = await fetch(
      "/api/v1/reader/chapters/" + chapter + "/open-confirmations",
      {
        method: "POST",
        credentials: "same-origin",
        redirect: "error",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
        body: JSON.stringify({ source: "layer", version_id: versionId }),
      }
    );
    if (opened.ok && opened.status === 200) {
      const receipt = await opened.json();
      openConfirmed = receipt.status === "recorded";
    }
  } catch { /* an absent ACK is UNKNOWN, not evidence of non-use */ }
  status.textContent = openConfirmed
    ? "Текущая редакция " + versionId + " · Глава открыта"
    : "Глава открыта, но подтверждение открытия не записано. Проверьте соединение.";
}

if (typeof document !== "undefined") {
  const root = document.querySelector("[data-layer-chapter-reader]");
  if (root) {
    startLayerReader(root).catch(() => {
      const status = root.querySelector("[data-layer-status]");
      if (status) status.textContent = "Доступ к главе недоступен. Проверьте подписку.";
      const article = root.querySelector("[data-layer-content]");
      if (article) article.replaceChildren();
    });
  }
}
