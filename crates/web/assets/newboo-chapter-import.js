'use strict';

// H-09/S023: preview remains editable; confirmation is durable and owner-scoped.
// No S3 key or signed URL is exposed to the browser.
(() => {
  const root = document.getElementById('chapter-import');
  if (!root) return;
  const bookId = root.dataset.bookId;
  const fileInput = document.getElementById('chapter-import-file');
  const encodingInput = document.getElementById('chapter-import-encoding');
  const textInput = document.getElementById('chapter-import-text');
  const previewButton = document.getElementById('chapter-import-preview');
  const confirmButton = document.getElementById('chapter-import-confirm');
  const cancelButton = document.getElementById('chapter-import-cancel');
  const results = document.getElementById('chapter-import-results');
  const status = document.getElementById('chapter-import-status');
  const csrfToken = document.getElementById('chapter-import-csrf').value;
  const commandKey = document.getElementById('chapter-import-command').value;
  let filename = '';
  let archiveBase64 = '';
  let accepted = null;
  let requestSerial = 0;
  let activeJob = null;

  function invalidate() {
    requestSerial++;
    accepted = null;
    confirmButton.disabled = true;
    results.replaceChildren();
  }

  function message(text) {
    status.textContent = text;
  }

  function sleep(milliseconds) {
    return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
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

  async function enqueue(payload) {
    const response = await fetch('/me/books/' + bookId + '/chapters/import-queue', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (response.status === 404) return null;
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(data.error || ('Ошибка сервера: ' + response.status));
      error.status = response.status;
      throw error;
    }
    return data;
  }

  async function latestJob() {
    const response = await fetch(
      '/me/books/' + bookId + '/chapters/import-jobs/latest',
      { credentials: 'same-origin' }
    );
    if (response.status === 404) return null;
    if (!response.ok) return null;
    return response.json().catch(() => null);
  }

  async function watchJob(jobId) {
    activeJob = String(jobId);
    const watched = activeJob;
    const query = '?job=' + encodeURIComponent(watched);
    window.history.replaceState(null, '', window.location.pathname + query);
    cancelButton.hidden = false;
    confirmButton.disabled = true;
    previewButton.disabled = true;
    fileInput.disabled = true;
    encodingInput.disabled = true;
    textInput.disabled = true;

    while (activeJob === watched) {
      let response;
      let data;
      try {
        response = await fetch(
          '/me/books/' + bookId + '/chapters/import-jobs/' + encodeURIComponent(watched),
          { credentials: 'same-origin' }
        );
        data = await response.json().catch(() => ({}));
      } catch {
        message('Потеряна связь с заданием. Проверка продолжится автоматически.');
        await sleep(2000);
        continue;
      }
      if (!response.ok) {
        if (response.status === 401 || response.status === 404) {
          activeJob = null;
          cancelButton.hidden = true;
          message('Задание недоступно. Обновите страницу и проверьте владельца книги.');
          return;
        }
        message('Не удалось получить состояние задания. Повторная проверка…');
        await sleep(2000);
        continue;
      }
      if (data.status === 'succeeded') {
        activeJob = null;
        cancelButton.hidden = true;
        message('Импорт завершён. Создано глав: ' + data.saved_count + '.');
        window.location.assign(data.book_url || ('/me/books/' + bookId));
        return;
      }
      if (data.status === 'failed') {
        activeJob = null;
        cancelButton.hidden = true;
        message('Импорт не выполнен (' + (data.error_code || 'ошибка') +
          '). Главы не сохранены. Обновите страницу, чтобы начать новую попытку.');
        return;
      }
      if (data.status === 'cancelled') {
        activeJob = null;
        cancelButton.hidden = true;
        message('Импорт отменён. Главы не сохранены. Обновите страницу для нового задания.');
        return;
      }
      message(data.status === 'running'
        ? 'Импорт выполняется. Можно отменить задание.'
        : 'Импорт стоит в очереди. Можно отменить задание.');
      await sleep(1500);
    }
  }

  async function readSelectedFile() {
    invalidate();
    previewButton.disabled = true;
    textInput.value = '';
    filename = '';
    archiveBase64 = '';
    const selected = fileInput.files && fileInput.files[0];
    if (!selected) {
      previewButton.disabled = true;
      message('Выберите файл.');
      return;
    }
    const lowName = selected.name.toLowerCase();
    if (selected.name.includes('/') || selected.name.includes('\\')
      || !(lowName.endsWith('.txt') || lowName.endsWith('.md')
        || lowName.endsWith('.docx') || lowName.endsWith('.zip'))) {
      previewButton.disabled = true;
      message('Разрешены только TXT, MD, DOCX и ZIP.');
      return;
    }
    const isDocx = lowName.endsWith('.docx');
    const isZip = lowName.endsWith('.zip');
    const isArchive = isDocx || isZip;
    if (selected.size > (isArchive ? 1250000 : 700000)) {
      previewButton.disabled = true;
      message(isArchive ? 'Архив больше 1,25 МБ.' : 'Файл больше 700 КБ.');
      return;
    }
    const serial = requestSerial;
    try {
      const bytes = await selected.arrayBuffer();
      if (isArchive) {
        if (serial !== requestSerial) return;
        const data = new Uint8Array(bytes);
        const blocks = [];
        for (let offset = 0; offset < data.length; offset += 8192) {
          blocks.push(String.fromCharCode(...data.subarray(offset, offset + 8192)));
        }
        archiveBase64 = btoa(blocks.join(''));
        filename = selected.name;
        previewButton.disabled = false;
        message((isZip ? 'ZIP' : 'DOCX')
          + ' прочитан. Проверьте главы, затем сможете исправить извлечённый текст.');
        return;
      }
      // Legacy code pages are converted explicitly to canonical UTF-8.
      const decoded = new TextDecoder(encodingInput.value, { fatal: true }).decode(bytes);
      if (serial !== requestSerial) return;
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
    try {
      const data = await send('import-preview', {
        filename, text: textInput.value, csrf_token: csrfToken,
        ...(archiveBase64 ? {
          file_base64: archiveBase64,
          archive_encoding: encodingInput.value,
        } : {}),
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
      if (archiveBase64) {
        if (typeof data.extracted_text !== 'string') throw new Error('Нет текста архива');
        textInput.value = data.extracted_text;
        filename = filename.replace(/\.(docx|zip)$/i, '.txt');
        archiveBase64 = '';
      }
      accepted = { filename, text: textInput.value, fingerprint: data.fingerprint };
      confirmButton.disabled = false;
      message('Найдено глав: ' + data.count + '. После подтверждения весь пакет сохранится одной операцией.');
    } catch (error) {
      if (serial === requestSerial) {
        message('Не удалось проверить файл: ' + error.message);
      }
    } finally {
      previewButton.disabled = false;
    }
  });

  cancelButton.addEventListener('click', async () => {
    if (!activeJob) return;
    cancelButton.disabled = true;
    try {
      const response = await fetch(
        '/me/books/' + bookId + '/chapters/import-jobs/' +
          encodeURIComponent(activeJob) + '/cancel',
        {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ csrf_token: csrfToken }),
        }
      );
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || 'Не удалось отменить задание.');
      if (data.cancelled) {
        activeJob = null;
        cancelButton.hidden = true;
        window.history.replaceState(null, '', window.location.pathname);
        message('Импорт отменён. Главы не сохранены. Обновите страницу для нового задания.');
      } else {
        message('Задание уже завершилось или было отменено. Проверяю итог…');
        await watchJob(activeJob);
      }
    } catch (error) {
      message(error.message);
    } finally {
      cancelButton.disabled = false;
    }
  });

  confirmButton.addEventListener('click', async () => {
    if (!accepted) return;
    confirmButton.disabled = true;
    previewButton.disabled = true;
    message('Отправка задания в очередь…');
    const payload = {
      ...accepted, csrf_token: csrfToken, command_key: commandKey,
    };
    try {
      let queued;
      try {
        queued = await enqueue(payload);
      } catch (error) {
        if (error.status === 409) {
          const latest = await latestJob();
          if (latest && latest.job_id) {
            await watchJob(latest.job_id);
            return;
          }
        }
        throw error;
      }
      if (queued) {
        message('Задание №' + queued.job_id + ' поставлено в очередь.');
        await watchJob(queued.job_id);
        return;
      }
      // Worker is not configured: retain the previously accepted synchronous
      // path rather than silently pretending that a queue is running.
      const data = await send('import-confirm', payload);
      message('Создано глав: ' + data.saved + '. Возвращаемся к книге.');
      window.location.assign(data.book_url);
    } catch (error) {
      message('Импорт не начат: ' + error.message
        + '. Проверьте соединение или обновите страницу.');
      confirmButton.disabled = false;
      previewButton.disabled = false;
    }
  });

  const jobFromUrl = new URLSearchParams(window.location.search).get('job');
  if (jobFromUrl && /^\d+$/.test(jobFromUrl)) {
    watchJob(jobFromUrl);
  } else {
    latestJob().then((latest) => {
      if (latest && latest.job_id) watchJob(latest.job_id);
    }).catch(() => {});
  }
})();
