(() => {
  'use strict';
  const form = document.querySelector('form[action="/me/books"]');
  const output = document.getElementById('book-title-suggestions');
  if (!form || !output) return;
  const fields = ['title', 'original_title', 'alt_title_1', 'alt_title_2', 'alt_title_3'];
  let timer, controller, generation = 0;
  const update = () => {
    clearTimeout(timer);
    if (controller) controller.abort();
    const current = ++generation;
    const values = fields.map(name => form.elements.namedItem(name)?.value.trim() || '');
    output.replaceChildren();
    if (!values.some(value => Array.from(value).length >= 3)) return;
    timer = setTimeout(async () => {
      controller = new AbortController();
      const params = new URLSearchParams({ language: form.elements.namedItem('target_language').value });
      ['title', 'original', 'alt1', 'alt2', 'alt3'].forEach((key, index) => params.set(key, values[index]));
      try {
        const response = await fetch('/me/books/title-suggestions?' + params, {
          credentials: 'same-origin', signal: controller.signal, cache: 'no-store',
        });
        if (!response.ok || current !== generation) return;
        const data = await response.json();
        if (current !== generation || !Array.isArray(data.items)) return;
        const paragraph = document.createElement('p');
        paragraph.textContent = data.items.length
          ? 'В каталоге есть похожие книги. Проверьте их перед регистрацией:'
          : 'В открытом каталоге похожих названий не найдено.';
        output.append(paragraph);
        const list = document.createElement('ul');
        for (const item of data.items.slice(0, 6)) {
          if (!/^[1-9][0-9]*$/.test(String(item.id))) continue;
          const link = document.createElement('a');
          link.href = '/books/' + item.id;
          link.textContent = item.title;
          const row = document.createElement('li');
          row.append(link);
          list.append(row);
        }
        output.append(list);
      } catch (error) {
        if (error.name !== 'AbortError' && current === generation) {
          output.textContent = 'Проверка названий сейчас недоступна. Книгу всё равно можно отправить.';
        }
      }
    }, 400);
  };
  for (const name of [...fields, 'target_language']) {
    form.elements.namedItem(name)?.addEventListener('input', update);
    form.elements.namedItem(name)?.addEventListener('change', update);
  }
})();
