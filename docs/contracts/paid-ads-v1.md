# S042 / S043 — оплаченная реклама TOR V1

Реализация сохраняет два отдельных продукта: дни в `ad_legacy_campaigns` и
пакеты показов в `ad_chapter_campaigns`. Обе покупки полностью предоплачены.
Старые credit/refill/shift-механизмы не переносятся. Внешнего ad-provider нет.
Тарифы, доли, hold и лимиты оператор публикует явно; миграции их не создают.

Основания: [ADS-01…16](../roadmap/ads-execution-contract.md),
[FIN-A](finance-ad-exact-v1.md), [API v1](../roadmap/api-v1.md),
[S042](https://app.notion.com/p/40432a84beec81378f36e7e103841e6c),
[S043](https://app.notion.com/p/40432a84beec8126a746ee659e18bf02).

## Финансовая граница

`MoneyMinor` и `LedgerDelta` находятся в `tor-contracts::money`; прежний
`tor-commerce::domain::money` экспортирует те же типы. Валюта — RUB, цена и
баланс — целые копейки; только delta может быть отрицательной. Cumulative
floor рассчитывается в D через i128. Бюджет 101/4 даёт расходы 25/25/25/26,
при доле издателя 7000 bps — 17/18/17/18. После двух показов возврат 51,
издателю 35, площадке 15. Нулевые дельты учитывают показ без нулевых wallet entries.

F вызывает `AdFundingPort`; прямого Cargo ads→commerce нет. D в одной PG
транзакции изменяет кошелёк, свой журнал/escrow и строго указанные строки F:
покупку кампании, reservation, settlement marker, окончание отмены. Это
исключение атомарности ADS-03/06/09/11/12, а не общий доступ F к финансам.

Purchase intent сохраняется до финансовой транзакции. Actor+key связан с
нормализованным запросом во всех pending/failed/posted состояниях. Posted
replay читает исходную квитанцию с доказательством канонического списания.
Цена, package revision, сроки, share, hold и caps после покупки неизменяемы.
Отдельные отложенные PG-проверки требуют баланса проводки и точной связи
intent/campaign/wallet/reservation/settlement/publisher credit/refund.

Отмена сначала фиксирует неизменяемое намерение и закрывает приём показов.
Подтверждённые reservations рассчитываются порциями до 100. Только затем
остаток возвращается исходному рекламодателю и появляется `cancelled`.
Recovery продолжает уже разрешённый возврат после истечения сессии и после
выключения продаж; новых списаний этот путь не создаёт.

Доход издателя проходит pending→available по captured clear_at→wallet.
Перевод использует отдельный durable command key и фиксирует исходный
результат даже для нулевого доступного остатка. Это внутренний перевод;
банковская выплата не выполняется.

## Выдача и право на показ

Используются существующие три места: home_featured, book_detail_related,
chapter_end_free. Текущая home_promo_strip сохраняется как отдельное
редакционное место с существующим fallback; платная реклама автоматически
не получает этот новый слот. На главной остаётся общий бюджет 400 ms и один
промоблок. Приоритет платного источника для разрешённого места публикуется
явной slot policy; отсутствие policy означает отсутствие платной выдачи.

Полка под бесплатной главой остаётся одним физическим блоком до пяти книг.
Оплаченные книги исключаются из органического заполнения, не дублируются и
имеют явную пометку «Реклама». Редакционный и органический контуры не создают
денежных проводок. Дневная выдача V1 получает только проверенную сессию,
durable day/hour/session caps и опубликованную минимальную паузу; выбор
редакционного приоритета не расходует эти caps.

Chapter planner работает по bounded keyset batches: до 100 host ID и до
200 candidate campaign ID на host. Cursor и assignments фиксируются одной
транзакцией с PG fence; active pointer меняется лишь после полного прохода.
Ошибка нового поколения оставляет старое активным. HTTP не сканирует все
назначения. Перед выдачей и при резервировании повторяются canonical Book
discovery/возраст/Hide, ownership, free public chapter, publisher opt-in,
активная сессия, generation и запрет self-ad. Рекламодатель не получает
права на чтение оплаченного или скрытого текста.

Opaque opportunity содержит серверные host/chapter/assignment/session facts;
в БД хранится hash токена. Клиент сообщает campaign/generation/assignment,
видимую долю и длительность. D сверяет все связи, серверное истечение и
минимум секунды, затем атомарно выполняет dedup+caps+hard target+reservation.
DOM render сам по себе не является оплачиваемым показом. Browser observer
сбрасывает таймер при потере видимости/фокуса. Серверные проверки ограничивают
подделку и повтор; утверждения о безошибочном распознавании человека нет.

## Метрики и данные

Click и canonical book open отделены от оплаты показа. Read/library/purchase
засчитываются только по уже committed C/D фактам после настоящего click context,
в окне до 30 минут, с уникальным outcome. Клиентского conversion endpoint нет.
Редакторские FLOW observations не используются как денежное доказательство.

Кабинет читает owner-index keysets до 100 кампаний/начислений и materialized
balances/stats. На GET нет lifetime SUM по журналу или всей почасовой истории.
При недоступной книге история платежа сохраняется, текущий скрытый title
не раскрывается. Токен и session binding не попадают в URL или журналы.
Fraud buckets имеют TTL; через сутки после opportunity expiry viewer/session
анонимизируются. Финансовое доказательство сохраняется без этих идентификаторов;
ненужные истёкшие opportunities удаляются bounded cleanup. Raw IP и новая
персональная история чтения не собираются.

## Интерфейс и включение

- `/me/ads`: advertiser dashboard, fully prepaid формы, отмена, publisher settings,
  pending/available/credited и перевод в кошелёк.
- `/admin/promotion/paid`: публикация неизменяемых тарифов/пакетов, slot policy,
  отмена через поддержку с причиной. Moderator не получает эти права.
- JSON использует отдельные daily/chapter endpoints из API v1, decimal string IDs,
  UTC RFC3339, integer minor money, bounded body, cookie Origin+CSRF и единственный
  `Idempotency-Key` в заголовке. HTML использует hidden `command_key` того же D use case.

Все флаги по умолчанию OFF, включение только точным значением `1`:

| Флаг | Разрешает |
| --- | --- |
| TOR_ADS_DAILY_PURCHASE_ENABLED | Новые покупки дней |
| TOR_ADS_DAILY_SERVING_ENABLED | Дневную выдачу |
| TOR_CHAPTER_ADS_PURCHASE_ENABLED | Новые пакеты показов |
| TOR_CHAPTER_ADS_PLANNER_ENABLED | Планирование поколений |
| TOR_CHAPTER_ADS_SERVING_ENABLED | Карточки Chapter Ads |
| TOR_CHAPTER_ADS_BILLABLE_ENABLED | Приём оплачиваемых показов |
| TOR_CHAPTER_ADS_SETTLEMENT_ENABLED | Общий settlement и maturity worker |

Отключение продаж/выдачи не запрещает исполнить существующую отмену или
получить уже заработанный доступный доход. Настоящие цены и условия требуют
публикации оператором. Изменение кода не включает флаги, не выполняет рабочие
миграции и не является деплоем.

## Проверки и трассировка

| Пакеты | Исполняемое доказательство |
| --- | --- |
| ADS-01/02/03/05/06 | Общие типы; daily/chapter purchase; raced replay, failed payload conflict, overlap, full prepayment, no overdraft/orphan campaign |
| ADS-04/07/10 | Publisher opt-in, staged generation pointer, current ACL, persistent caps/minimum interval and editorial priority |
| ADS-08/09 | Real Chromium 57% vs 60%/1s, forged actor/context, replay; 20 parallel events at 98/100 accept exactly two |
| ADS-11/12/13 | Concurrent settlement and publisher transfer; 72h hold; exact refund; zero-cost events and balanced deferred evidence |
| ADS-14/15 | Click/open and actual C/D completion/library/checkout outcomes; owner denial, 100k hourly rows and bounded dashboard |
| ADS-16 | `tools/paid_ads_browser.cjs`: native daily+chapter purchase→viewability→settlement→refund→publisher wallet, 390/1440 and CSP |

Команды: `cargo test -p tor-commerce --test paid_ads_pg -- --ignored --test-threads=1`,
`cargo test -p tor-web --test paid_ads_http_pg` с раздельными explicit-on и
default-off запусками в CI, а также `tools/paid_ads_browser.cjs`. Тесты допускают
только throwaway `/tor_ci`. Required/manual PG и reader-browser workflows
содержат одинаковые команды; RIL привязан к точным handler/source bindings и
имеет отрицательные тесты удаления actor/admin/CSRF/default-off guards.
