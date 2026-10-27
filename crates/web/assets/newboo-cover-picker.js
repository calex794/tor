/* Local adaptation of NEwBOO backend/app/web/static/js/book-settings.js.
 * Only the cover picker; no external requests or inline executable content. */
(() => {
  "use strict";
  for (const picker of document.querySelectorAll("[data-cover-picker]")) {
    const input = picker.querySelector("[data-cover-picker-input]");
    const image = picker.querySelector("[data-cover-picker-image]");
    const placeholder = picker.querySelector("[data-cover-picker-placeholder]");
    const filename = picker.querySelector("[data-cover-picker-filename]");
    const error = picker.querySelector("[data-cover-picker-error]");
    if (!input || !image || !placeholder || !filename || !error) continue;
    let preview = null;
    input.addEventListener("change", () => {
      if (preview) {
        URL.revokeObjectURL(preview);
        preview = null;
      }
      const file = input.files && input.files.length ? input.files[0] : null;
      error.textContent = "";
      if (!file) return;
      if (file.size > 8 * 1024 * 1024 || file.size === 0 ||
          !["image/jpeg", "image/png", "image/webp"].includes(file.type)) {
        error.textContent = "Выберите изображение JPG, PNG или WebP до 8 МиБ.";
        input.value = "";
        return;
      }
      preview = URL.createObjectURL(file);
      image.src = preview;
      image.hidden = false;
      placeholder.hidden = true;
      filename.textContent = file.name;
      picker.classList.add("book-cover-picker--selected");
    });
    window.addEventListener("pagehide", () => {
      if (preview) URL.revokeObjectURL(preview);
    }, { once: true });
  }
})();
