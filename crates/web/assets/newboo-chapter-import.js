'use strict';

// H-09: source text stays in memory and is sent to the same-origin owner route.
// No scripts are inlined and all untrusted text uses textContent, never HTML.
(() => {
  const root = document.getElementById('chapter-import');
  if (!root) return;
  const bookId = root.dataset.bookId;
  const fileInput = document.getElementById('chapter-import-file');
  const encodingInput = document.getElementById('chapter-import-encoding');
  const textInput = document.getElementById('chapter-import-text');
  const previewButton = document.getElementById('chapter-import-preview');
  const confirmButton = document.getElementById('chapter-import-confirm');
  const results = document.getElementById('chapter-import-results');
  const status = document.getElementById('chapter-import-status');
  const csrfToken = document.getElementById('chapter-import-csrf').value;
  const commandKey = document.getElementById('chapter-import-command').value;
  let filename = '';
  let docxBase64 = '';
  let accepted = null;
  let requestSerial = 0;

  function invalidate() {
    requestSerial++;
    accepted = null;
    confirmButton.disabled = true;
    results.replaceChildren();
  }

  function message(text) {
    status.textContent = text;
  }

  async function send(path, payload) {
    const response = await fetch('/me/books/' + bookId + '/chapters/' + path, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw new Error(data.error || ('Ошибка сервера: ' + response.status));
    }
    return data;
  }

  async function readSelectedFile() {
    invalidate();
    previewButton.disabled = true;
    textInput.value = '';
    filename = '';
    docxBase64 = '';
    const selected = fileInput.files && fileInput.files[0];
    if (!selected) {
      previewButton.disabled = true;
      message('Выберите файл.');
      return;
    }
    const lowName = selected.name.toLowerCase();
    if (selected.name.includes('/') || selected.name.includes('\\')
      || !(lowName.endsWith('.txt') || lowName.endsWith('.md') || lowName.endsWith('.docx'))) {
      previewButton.disabled = true;
      message('Разрешены только TXT, MD и DOCX.');
      return;
    }
    const isDocx = lowName.endsWith('.docx');
    if (selected.size > (isDocx ? 1250000 : 700000)) {
      previewButton.disabled = true;
      message(isDocx ? 'DOCX больше 1,25 МБ.' : 'Файл больше 700 КБ.');
      return;
    }
    const serial = requestSerial;
    try {
      const bytes = await selected.arrayBuffer();
      if (isDocx) {
        if (serial !== requestSerial) return;
        // Encode a bounded file; no external JS or third-party upload service.
        const data = new Uint8Array(bytes);
        const blocks = [];
        for (let offset = 0; offset < data.length; offset += 8192) {
          blocks.push(String.fromCharCode(...data.subarray(offset, offset + 8192)));
        }
        docxBase64 = btoa(blocks.join(''));
        filename = selected.name;
        previewButton.disabled = false;
        message('DOCX прочитан. Проверьте главы, затем сможете исправить извлечённый текст.');
        return;
      }
      // A legacy code page is used only after explicit selection by the author.
      const decoded = new TextDecoder(encodingInput.value, { fatal: true }).decode(bytes);
      if (serial !== requestSerial) return;
      // The Rust parser applies its 700 kB limit to the UTF-8 text sent as JSON.
      if (new TextEncoder().encode(decoded).byteLength > 700000) {
        previewButton.disabled = true;
        message('После преобразования в UTF-8 текст больше 700 КБ.');
        return;
      }
      filename = selected.name;
      textInput.value = decoded;
      previewButton.disabled = false;
      message('Файл прочитан. Проверьте главы.');
    } catch {
      if (serial !== requestSerial) return;
      previewButton.disabled = true;
      message('Файл повреждён или имеет другую кодировку. Выберите кодировку вручную.');
    }
  }

  fileInput.addEventListener('change', readSelectedFile);
  encodingInput.addEventListener('change', readSelectedFile);

  textInput.addEventListener('input', invalidate);

  previewButton.addEventListener('click', async () => {
    if (!filename) {
      message('Сначала выберите файл.');
      return;
    }
    invalidate();
    const serial = requestSerial;
    previewButton.disabled = true;
    message('Проверка границ глав…');
    const text = textInput.value;
    try {
      const data = await send('import-preview', {
        filename, text, csrf_token: csrfToken,
        ...(docxBase64 ? { file_base64: docxBase64 } : {}),
      });
      if (serial !== requestSerial) return;
      const list = document.createDocumentFragment();
      for (const chapter of data.chapters) {
        const item = document.createElement('li');
        const title = document.createElement('strong');
        title.textContent = chapter.title;
        const excerpt = document.createElement('p');
        excerpt.textContent = 'Символов: ' + chapter.characters
          + '. Начало: ' + chapter.excerpt;
        item.append(title, excerpt);
        list.append(item);
      }
      results.replaceChildren(list);
      if (docxBase64) {
        if (typeof data.extracted_text !== 'string') throw new Error('Нет текста DOCX');
        // Reuse the canonical UTF-8 text import on confirmation and after edits.
        textInput.value = data.extracted_text;
        filename = filename.replace(/\.docx$/i, '.txt');
        docxBase64 = '';
      }
      accepted = { filename, text: textInput.value, fingerprint: data.fingerprint };
      confirmButton.disabled = false;
      message('Найдено глав: ' + data.count
        + '. Все будут созданы в черновике одной операцией.');
    } catch (error) {
      if (serial === requestSerial) {
        message('Не удалось проверить файл: ' + error.message);
      }
    } finally {
      previewButton.disabled = false;
    }
  });

  confirmButton.addEventListener('click', async () => {
    if (!accepted) return;
    confirmButton.disabled = true;
    previewButton.disabled = true;
    message('Сохранение всех глав одной операцией…');
    try {
      const data = await send('import-confirm', {
        ...accepted, csrf_token: csrfToken, command_key: commandKey,
      });
      message('Создано глав: ' + data.saved + '. Возвращаемся к книге.');
      window.location.assign(data.book_url);
    } catch (error) {
      message('Сохранение не завершено: ' + error.message
        + '. Можно повторить с тем же файлом.');
      confirmButton.disabled = false;
      previewButton.disabled = false;
    }
  });
})();
