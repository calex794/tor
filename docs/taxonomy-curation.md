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
python3 tools/taxonomy_admin.py lookup tag 'сис' --limit 40
python3 tools/taxonomy_admin.py --confirm-db tor_ci add tag 'Система' --curator review-20261101
python3 tools/taxonomy_admin.py --confirm-db tor_ci alias tag 42 'System' --curator review-20261101
python3 tools/taxonomy_admin.py reconcile 123 --curator review-20261101
python3 tools/taxonomy_admin.py --confirm-db tor_ci reconcile 123 --apply --curator review-20261101
```

`add` creates the immutable canonical ID or returns an existing ID only when
the exact label and active status match. Repeated approved CSV imports cannot
duplicate audit records. Alternative spellings, inactive terms and conflicts
with another term's alias are rejected instead of being silently renamed.
`alias` maps a normalized spelling
to exactly one active term in the same kind. Repeated identical aliases are
idempotent. The same spelling cannot belong to a different canonical term
or collide with another canonical name. Both operations have migration-owner
audit entries; app role has no write or function-execution permissions.

`unmapped` reports **only public-book** labels that have no active canonical
or alias match. Use `--offset` with `--limit` to review more than 500
rows; order is by book count, kind and name. Avoid dictionary edits between
pages if a stable multi-page review is needed. `lookup kind prefix` searches
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

По умолчанию операция только проверяет CSV: база не подключается.
Запись требует `--apply`, подтверждения имени базы и миграционной роли.
Повторная загрузка идентичного утверждённого CSV допустима; новые проверенные
алиасы можно добавлять к существующему активному термину только при точном
совпадении исходного канонического названия. Один конфликт
(алиас/название/тип) откатывает **весь** импорт.

Нормализация сейчас включает нижний регистр, последовательности пробелов
и русское «ё»/«е». **Unicode NFKC не реализован**: похожие совместимые
символы не объединяются автоматически и требуют редакционной проверки.
Ограничения:
до 10 000 терминов, 10 алиасов для каждого, 5 МБ на файл.

**Not implemented:** automatic tag merging, a web staff panel, bulk private
book scanning, default AI-generated tags, or production rollout. These
require explicit product and staff-authority decisions. Repo workflows
run only on disposable `tor_ci`; main/prod remain untouched.
