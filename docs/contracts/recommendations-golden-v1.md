# TOR REC v1 — эталон данных, расчётов, поколений, RIL и паритета (PREF-04)

**Технический выбор:** ничего не обучать заново и не заменять исходный гибридный алгоритм `ORDER BY popularity`. F переносит чистые вычисления из [Python scoring](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/services/hybrid_scoring_service.py) и согласует с [RIL/progress](ril-reader-signals-v1.md), [планом REC-01…14](../roadmap/recommendations-execution-contract.md). I закрепляет версию `recommendations_v1`, G только фоновые jobs, H витрины. [Проверяемые ручные fixture-входы и результаты](rec-golden-v1.json) — не утверждение, что Python был запущен в текущем окружении. При переносе исполнитель ОБЯЗАН добавить прогон оригинальной функции NEwBOO на зафиксированном SHA и сравнить с этим оракулом; любое отличие — явно объяснить, не скрыто менять ожидаемое число.

## 1. Однозначная формула и сортировка

Источники: `behavioral`, `content`, `popularity`; `related_books` — отдельные вручную связанные книги, не участвуют в hybrid весах. Каждый кандидат имеет `(book_id,source,reason,normalized_score)`, score целое положительное 1..1000. Веса `(wb,wc,wp)` целые ≥0, `wb+wc+wp=100`. Для одинаковой книги и source выбирается наиболее сильная пара по contribution, при tie — стабильный ключ (Python reason/source/book, см. исходный файл). Вклад по источнику: `floor(normalized_score*weight/100)`. Сумма лучших вкладов разных источников одной книги + bounded verified quality boost (0..50) → clamp 0..1000. Итог сортировать `final_score DESC, book_id ASC`, источники/reasons и label — по Python детерминированному приоритету; cap популярных выходов 10, limit ≤50 (HTTP рекомендательно 20).

**Ручной оракул (fixture REC-M01):**

| Книга | Behavioral ×50% | Content ×30% | Popularity ×20% | Score |
| --- | ---: | ---: | ---: | ---: |
| A | 800→400 | 700→210 | 500→100 | **710** |
| B | 600→300 | 800→240 | 400→80 | **620** |
| D | 500→250 | 600→180 | – | **430** |
| C | – | – | 1000→200 | **200** |

Очередь A,B,D,C; повтор `A/behavioral 600` не прибавляет вторую сумму. Веса 50+30+19 → `invalid_weights`, score=0 → `missing_normalized_scores`. Два равных итоговых результата отсортировать по stringified stable BookId ASC, но цифровые BookId в контракте — десятичные строки: при 2 vs 10 нужна **явная** typed numeric ID comparison; паритет Python сортировал строкой, поэтому для v1 фиксировать лексикографический порядок как в исходном коде, менять только в `algorithm_version=v2`.

**Пределы из исходного кода:** hybrid candidates≤500, max results≤50, max popularity in result=10, quality boost≤50, bounded final≤1000; recent books≤5, per source behavioral/content≤20, related≤10, popularity/fallback≤20, metadata fetch≤100, response limit≤20. Эти значения — базовая совместимость, **конфигурируемые лимиты без нарушения security/нагрузки**, не произвольные пороги на 1GiB VPS.

## 2. Новичок и приватность

- Cold start с **0 пользователей**: строить content similarity, metadata/genre, newly published и public popularity по **существующим книгам**, без фиктивных read/completion events. Если каталог содержит одну публичную книгу, fallback может вернуть её только в уместной поверхности; скрытый/private/adult-underage не показывать.
- Для каждого HTTP ответа применять актуальную `CanDiscover` книгу/возрастные ограничения/модерацию **после** чтения активного поколения и перед отдачей DTO. Инвалидировать все пять поверхностей (`home,book_detail,reader_end,catalog,library`); stale projection не выдаёт скрытую книгу.
- Рекламные события `AdImpressionConfirmed`/paid placement **не** становятся органическим `reader_opened` или `recommendation_score`. Только реальные event sources из типизированного бизнес-владельца.
- RIL версионирует только *классификацию HTTP маршрута*. S3 GET/выданный grant не доказывает чтение. `ChapterCompleted` создаётся C по immutable manifest B/точной версии и PG-CAS и весит 700; G доставляет с original RoutePassport/provenance; F не классифицирует worker URL заново.
- Из 8 допустимых категорий событий [RIL v1](ril-reader-signals-v1.md) брать веса 100/150/250/300/700/800/1000/1000 в соответствующих переходах. Событие без доверенного источника → не обучает; при обработке вслепую не подставлять `trusted_internal` от клиента.

## 3. Materialization: отдельные write-path и serving-path

`rec_generations(id,algorithm_version,started_at,finished_at,manifest_hash,status,source_window_end)`; `rec_candidate_edges(generation_id,source_book_id,target_book_id,source,score)`; `rec_active_generation` одна строка с `generation_id`. G job с lease/fence строит новое поколение в ``building`, валидирует источники, quality, `CanDiscover` и на одной PG транзакции переключает активный pointer из `validated→active`. Rejection/timeout сохраняет предыдущую active generation. HTTP не выполняет full-graph rebuild. Предыдущие поколения удалять только после expiry, cursor/window/bounded GC; скрытие B применяется live независимо от отложенного backfill.

**Rollout:** preserve Python stages `1→5→10→25→50→100%`; stable hash(subject,experiment_key,algorithm_version) → cohort/holdout, no raw user ID in metrics labels. На stage должны быть метрики `p95 query time`, число SQL запросов, `latency/errors`, ratio пустых ответов, leaked hidden=0, duplicate-source=0. При отсутствии достаточной фактической аудитории experiment uplift **не доказан**, не объявлять «повысили удержание»; только техническое hold/rollback. Holdout не обучать фальшивой историей.

## 4. Детерминированный реестр тестов — 12 блоков

| ID | Вход/отказ | Выход |
| --- | --- | --- |
| REC-M01 | три источника, 4 книги, веса 50/30/20 | 710,620,430,200; порядок A,B,D,C |
| REC-M02 | два behavioral кандидата A, 800 и 600 | 800 выигрывает, не две суммы |
| REC-M03 | веса 50/30/19 и отрицательный компонент | rejection `invalid_weights` |
| REC-M04 | 501 candidate и limit=51 | bounded rejection, не unbounded |
| REC-M05 | score отсутствует или ≤0, неверный BookId | reject/filter согласно Python original, без panic |
| REC-M06 | related_books manually_related вместе с hybrid | отдельный канал, никогда double count |
| REC-M07 | popularity 50/50 во всех source | ≤10 output best_source=popularity |
| REC-M08 | одна книга получена по двум источникам и +50 quality | сумма вклада +50, clamp≤1000 |
| REC-M09 | новый пользователь/0 events | deterministic public content/popularity fallback, no invented reads |
| REC-M10 | published→hidden между generation и GET | 0 hidden results во всех пяти ответах |
| REC-M11 | S3 grant/GET, mid progress, valid final manifest block, retry+V2 | только финал создаёт 1 ChapterCompleted/700 |
| REC-M12 | G restart, outbox delayed, trusted RIL passport vs forged payload | 1 trusted rec event, 0 forged, previous generation remains on failed build |

**Как исполнитель должен проверять:** (a) Rust pure unit для calc/limits/ties; (b) Python fixture на **реальном pinned исходнике** с freeze окружения; (c) PG projection/rollout + 2 actor, (d) S3/Chromium paid reader, (e) query plan/latency в малом VPS. CI прикладывает исходные значения/expected/Rust output/diff и SHA. Ручной оракул REC-M01 математически задан, но не заменяет запуск сложных правил Python (scenario weights, quality, tie). Не считать tests passed по описанию.

## Проверяемый перенос REC-07 — 31.10.2026

`tools/check_hybrid_parity.py` исполняет неизменённые снимки Python на указанном выше SHA; Git blob SHA и SHA-256 записаны в `tests/fixtures/newboo_hybrid/manifest.json`. Неигнорируемый Rust-тест `hybrid_parity` сравнивает все поля результата на 181 случае. Это проверка чистого scoring, не приёмка PG/HTTP/пяти поверхностей.

Исправление REC-M08: старое `quality_score_by_book_id: 50` ошибочно трактовалось документным валидатором как готовый boost +50. В исходной функции raw quality 50 даёт +2 (`50 // 20`), а raw quality 1000 даёт +50. В fixture теперь raw quality 1000, ожидаемый итог 760 сохранён; валидатор применяет исходную формулу. Отдельный дифференциальный случай проверяет raw quality 50.
