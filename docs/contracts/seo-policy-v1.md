# SEO-01 / F-08 / S060: единая политика индексации TOR

Статус: чистые доменные правила и испытания в PR. Не активирует индексацию,
не регистрирует HTTP-маршруты и не меняет рабочий VPS.

Источники:
- NEwBOO: https://github.com/HeisLuka/newboo/blob/main/backend/app/services/seo/book_policy.py
- NEwBOO: https://github.com/HeisLuka/newboo/blob/main/backend/app/services/seo/route_contract.py
- NEwBOO: https://github.com/HeisLuka/newboo/blob/main/backend/app/services/seo/metadata.py
- Notion: https://app.notion.com/p/40632a84beec81bc9fa7eba5030367f6
- TOR roadmap: docs/roadmap/tasks.tsv (F-08, F-13, H-10, S060)

## Граница

Правила реализованы в существующем tor-search/domain/seo и
tor-search/services/seo. Новый Cargo-пакет не нужен. Линия F отвечает
за решение indexable/noindex/sitemap, H только выводит решение в HTML/HTTP,
B/C остаются единственным источником публичных прав, I подключает маршруты
и выпуск, G отправляет события изменения публикации.

SeoVisibility::AnonymousPublic означает не простое значение visibility='public',
а уже проверенную доступность без авторизации по правилам B/C. В частности:
должны учитываться admin_hold, is_deleted, факт публикации версии, access_level,
reader_released_at и состояние родительской книги. Домен SEO ничего не читает
из базы и не выдаёт права. SeoBookFacts::has_public_content требуется для
публикуемой карточки; пустая книга не индексируется.

Единый результат SeoDecision:
- robots — IndexFollow, NoIndexFollow или NoIndexNoFollow;
- sitemap_included — только для подтверждённо индексируемых страниц;
- canonical_path — безопасный относительный путь только для них;
- reason — машинопроверяемая причина решения.

Абсолютный URL строится позже через доверенную конфигурацию TOR_PUBLIC_ORIGIN,
а не из HTTP Host/Forwarded. До этого этапа никаких абсолютных SEO-адресов,
robots.txt или sitemap.xml эта ветка не отдаёт.

## Матрица

| Маршрут / состояние | Public | Pilot | Disabled |
| --- | --- | --- | --- |
| GET / | index | noindex | noindex |
| GET /catalog без фильтров | index | noindex | noindex |
| GET /catalog с фильтрами/пагинацией | noindex | noindex | noindex |
| GET /books/{id}, проверена публичность и доступная глава | index если сайт владелец | index только после ручного допуска | noindex |
| GET /books/{id} с дополнительной страницей оглавления | noindex | noindex | noindex |
| GET /reader/{id}, проверена бесплатная опубликованная версия | index если сайт владелец | index только после ручного допуска | noindex |
| Черновик/блокировка/удаление/отложенная глава/платная версия | noindex, чтение решает B/C | noindex | noindex |
| Личные, API, служебные, SEO-артефакты и неизвестные | noindex | noindex | noindex |

Для второго сайта ручное shadow_enabled не означает index_owner=Shadow.
Два независимых переключателя; у одной книги только один SEO-владелец.
Пока PR Shadow №170 не объединён, поля отсутствуют в main и настоящего
веб-сайта Shadow нет. Они лишь представлены типами, без записи в PostgreSQL.

## Приёмка SEO-01

- Единая закрытая по умолчанию политика в tor-search.
- Точный whitelist известных шаблонов GET-маршрутов (не импортировать RIL).
- Rust-unit для public/pilot/disabled, книги, главы, платности, снятия
  публикации, отсутствия содержимого, двух доменов, нулевых ID и других путей.
- PostgreSQL-интеграция на изолированной tor_ci проверяет публикацию главы,
  отсутствие глав, задержку выпуска, блокировку и удаление книги.
- CI: Format/unit/Clippy + tor-search --test seo_pg после миграций.
- Не менять web/layout.html, Nginx, домены, release flags, текущий noindex.

## Следом

SEO-02: B/I добавляют описание книги в новую миграцию, реальную публичную
проекцию полей и единую проверку B/C как источник SeoBookFacts. После этого
SEO-03 фиксирует постоянные адреса, а H подключает metadata/head в SEO-04.
SEO-06/09 добавляют карты, проверку поколений, атомарную активацию и откат.

Невыполненное подключение HTTP и настоящая поисковая индексация не могут
считаться готовыми из-за наличия этих типов и тестов.
