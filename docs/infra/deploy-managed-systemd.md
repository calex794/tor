# DEPLOY-04 / I-11 — управляемая политика systemd

Расширение ещё не активированного формата v2: подписывается один
systemd-web-release.conf, из ops/systemd/40-tor-release.conf.
Узкий allowlist: Restart, RestartSec (1–60 s), NoNewPrivileges,
ProtectSystem, ProtectHome, PrivateTmp, PrivateDevices, UMask.
Никаких произвольных ExecStart, User, EnvironmentFile, root commands.
Параметры соответствуют существующему примеру tor-web.service.
Полный unit, 20-postgres.conf, 30-runtime.conf, секреты, лимиты памяти,
CPU и TasksMax остаются операторскими. Trace Store не включается
автоматически: его отдельная опциональная служба требует своего допуска.

UnitPolicy.apply проверяет подпись полного доверенного v2 пакета,
фиксированный формат drop-in и baseline identity. Передаёт в systemd
только фиксированные команды verify/daemon-reload/show; после reload
проверяет эффективные свойства, включая сохранение operator properties.
На отказ возвращает прежние байты или отсутствие файла, делает reload
и сверяет baseline. Receipt позволяет будущему общему координатору
откатить политику до возврата старого приложения.
Файл записывается атомарно, без редактирования иных drop-ins.

Это библиотека для будущего root executor, не новая sudoers команда.
Она не запускает и не останавливает приложение. Активация systemd
политики при фактическом restart и combined rollback — DEPLOY-05.
Пока receipt хранится в памяти: восстановление после SIGKILL/power loss
потребует durable journal. 10 тестов с реальными SSH-подписями и
имитированными командами systemd, без production credentials.
Не заменяют реальные systemd VM испытания. CI ещё не подтверждён.
