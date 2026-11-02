# B/TAX-03 — offline taxonomy curator

TOR currently has **no reviewed staff/editor role**. Do not expose this
capability through regular account cookies, operator diagnostics, or a new
public HTTP route. Only the independent database migration owner
`tor_migrator` may create canonicals, assign aliases, or reconcile metadata.

Use `TOR_TAXONOMY_ADMIN_URL` (no default) and explicit write confirmation:

```shell
export TOR_TAXONOMY_ADMIN_URL='postgresql://tor_migrator:...@127.0.0.1:5432/tor_ci'
python3 tools/taxonomy_admin.py unmapped --limit 40
python3 tools/taxonomy_admin.py unmapped --limit 40 --offset 40
python3 tools/taxonomy_admin.py variants tag 'система' --limit 50
python3 tools/taxonomy_admin.py lookup tag 'сис' --limit 40
python3 tools/taxonomy_admin.py --confirm-db tor_ci add tag 'Система' --curator review-20261101
python3 tools/taxonomy_admin.py --confirm-db tor_ci alias tag 42 'System' --curator review-20261101
python3 tools/taxonomy_admin.py reconcile 123 --curator review-20261101
python3 tools/taxonomy_admin.py --confirm-db tor_ci reconcile 123 --apply --curator review-20261101
```

`add` creates the immutable canonical ID or returns an existing ID only when
the exact label and active status match. Repeated approved CSV imports cannot
duplicate audit records. Public-book labels are staged as `candidate`
instead of `active`: visibility is NOT editorial approval. A curator
may approve an existing candidate using exactly the same displayed label;
the canonical ID is retained and an `approve_term` audit event is written.
The same approved CSV can be imported again without duplicate approvals.
Alternative spellings, blocked/merged terms and conflicts with another
term's alias are rejected instead of being silently renamed.
`alias` maps a normalized spelling
to exactly one active term in the same kind. Repeated identical aliases are
idempotent. The same spelling cannot belong to a different canonical term
or collide with another canonical name. Both operations have migration-owner
audit entries; app role has no write or function-execution permissions.

`unmapped` reads **only public** labels from the transactionally maintained
`public_catalog_facets` projection, never private books. It returns
`kind | review_label | normalized_key | label_uses | spellings | term_id | status`.
`review_label` is the exact existing candidate label when an ID is known;
`status` is `candidate`, `blocked`, `merged`, or `not_staged` for a label
not yet inserted into the canonical table. Only approved active terms
are excluded. `label_uses` sums per-spelling reference counts; it may be
greater than the count of distinct books if one book used several
Unicode-equivalent spellings. `spellings` is the number of raw variants.
Use `--offset` with `--limit` for pagination; ordering is by descending
label uses, kind and normalized key. Avoid edits between pages if a
stable multi-page review is needed. This view does not approve terms.
Use `variants kind label` to inspect **every original public spelling**
within one NFKC-normalized group, including its reference count. Output is
JSON Lines (one object per spelling) so names with punctuation, commas,
pipes and newlines cannot corrupt column boundaries. Both commands are
read-only, public-only, and allow pagination; neither exposes drafts.
`lookup kind prefix` searches
canonical names and approved aliases and returns canonical ID, label,
status and match type (including blocked entries). It is read-only; SQL
wildcards are not accepted. Neither command exposes private draft metadata.
The `reconcile` preview prints raw labels for a **single explicitly selected**
book. `reconcile --apply` takes an exclusive book row lock, converts all
genres/tags/fandoms through reviewed active canonical terms and aliases,
updates the legacy TEXT[] and canonical links atomically, increments the book
taxonomy revision, and records an audit fact **only if something changed**.
It refuses unknown/ambiguous/blocked labels, duplicate canonical IDs, >8
labels per kind, missing/deleted/held books. It never drops unknown terms
or rewrites unrelated books.

The curator name is an audit note; the **database role**, not that string,
is the authorization boundary. Do not pass secrets as curator labels.


### Пакетный импорт большого справочника

Сначала подготовить проверенный UTF-8 CSV с ровно тремя полями
`kind,label,aliases`. Несколько синонимов разделяются символом `|`
в третьей колонке.

```csv
kind,label,aliases
tag,Система,System|系统
tag,Реинкарнация,Reincarnation|転生
genre,Фэнтези,Fantasy
```

```sh
python3 tools/taxonomy_import_csv.py reviewed-tags.csv --curator review-20261101
TOR_TAXONOMY_ADMIN_URL='postgresql://tor_migrator:...@127.0.0.1:5432/tor_ci' \
  python3 tools/taxonomy_import_csv.py reviewed-tags.csv --curator review-20261101 \
  --confirm-db tor_ci --apply
```

Опубликованные книги сами по себе не утверждают словарь. При
первоначальной SQLx-миграции **уже существующие** публичные метки становятся
кандидатами со статусом `candidate`, но не появляются в подсказках автора.
Метки, опубликованные позднее, остаются в `public_catalog_facets` и
видны через read-only `unmapped`, пока их не одобрит куратор. Для допуска
в список нужен одобренный CSV или команда `add` с **точно совпадающим**
названием. Одобрение существующего кандидата сохраняет ID и журналируется;
новому термину назначается новый ID. Старые книги сопоставляются только
через явный `reconcile` по одной, без удаления неизвестных тегов.

Для проверки кандидатов без сканирования приватных книг используйте
`python3 tools/taxonomy_admin.py unmapped --limit 40 --offset 0`.
Вывод: `kind|review_label|normalized_key|label_uses|spellings|term_id|status`.
Для просмотра всех вариантов одного ключа запустите
`python3 tools/taxonomy_admin.py variants tag 'система' --limit 50`.
Команда возвращает JSON Lines с исходными написаниями и их употреблениями
только из публичного каталога. Никакое написание не одобряется автоматически.
Если запись была создана при миграции, `review_label` — её исходное
написание, которое можно одобрить без изменения ID. Для `not_staged`
пока нет ID. `label_uses` — сумма употреблений разных написаний,
а не обязательно количество уникальных книг.

Поэтому пустой новый словарь нельзя считать готовой базой терминов:
до проверки и импорта реальных данных подсказки могут быть пустыми.

По умолчанию операция только проверяет CSV: база не подключается.
Запись требует `--apply`, подтверждения имени базы и миграционной роли.
Повторная загрузка идентичного утверждённого CSV допустима; новые проверенные
алиасы можно добавлять к существующему активному термину только при точном
совпадении исходного канонического названия. Один конфликт
(алиас/название/тип) откатывает **весь** импорт.

Ключи в PostgreSQL приводятся к Unicode NFKC, нижнему регистру,
сворачивают последовательности пробелов и считают русские «ё»/«е»
эквивалентными. Python-проверка CSV предварительно повторяет эти правила,
но PostgreSQL остаётся окончательным источником истины для локалезависимого
регистра. Отображаемое название **не переписывается**: изменения формы
канонической метки требуют редакционного решения. NFKC-эквивалентные
написания нельзя создать как отдельные канонические теги.

Поиск по буквенным префиксам (в том числе команда `lookup` куратора)
использует этот ключ. Символы, которые после NFKC становятся SQL-масками
(`%`, `_`, обратная косая черта), не разворачиваются в широкие запросы:
возвращается пустой список. Это проверяется непосредственно через
PostgreSQL, а не только предварительным анализом текста.
Ограничения:
до 10 000 терминов, 10 алиасов для каждого, 5 МБ на файл.

**Not implemented:** automatic tag merging, a web staff panel, bulk private
book scanning, default AI-generated tags, or production rollout. These
require explicit product and staff-authority decisions. Repo workflows
run only on disposable `tor_ci`; main/prod remain untouched.


### Стартовый словарь v1 — предложение для редакторской проверки

Файл [`starter-proposal-v1.csv`](../data/taxonomy/starter-proposal-v1.csv)
содержит **34 жанра, 110 сюжетных тегов и 151 варианта названий**.
Это новый редакторский черновик, **не экспорт и не подтверждённый список
NEwBOO**. Синонимы и деление на жанры/теги нужно принять или исправить
отдельным редакторским решением. Фэндомы намеренно не включены: канонические
названия франшиз требуют отдельной проверки.

```sh
# Проверка всего черновика без подключения к базе:
python3 tools/taxonomy_import_csv.py \
  data/taxonomy/starter-proposal-v1.csv --curator starter-review
```

Файл **не применяется** при обычном запуске сервера или выполнении
миграций. CI проверяет его полностью и дважды импортирует только в
одноразовую локальную `tor_ci`, контролируя идемпотентность, алиасы
и число активных терминов. Это *механическая приёмка*, а не факт
редакторского утверждения содержимого.

Перед использованием в реальном каталоге редактор сверяет `unmapped` и
`variants` с существующими значениями; согласовывает точные русские
названия, синонимы и конфликты с действующими ID. Для первой версии
англоязычные алиасы `Fantasy` и `Adventure` намеренно не привязаны
к `Фэнтези` и `Приключения`: такие канонические имена уже используются
как отдельные тестовые записи и не должны молча сливаться. После утверждения
отредактированный CSV можно применять через `--confirm-db` и `--apply`
с правами `tor_migrator`; каждое действие будет журналироваться.
