# Delivery and recovery / Доставка и восстановление

## Русский

Daemon сохраняет нормализованное событие и его доставки одной SQLite-транзакцией до отправки. Ответ Telegram и следующий polling offset также сохраняются вместе. Прогресс неподдерживаемых или запрещённых updates сохраняется без копирования их текста. Реальный источник в #4: событие запуска сервиса; остальные collectors добавляются последующими тикетами.

Получатель текущей установки: локально привязанный владелец в личном чате. Внутри очереди состояние каждого получателя независимо. До четырёх отправок выполняются одновременно, не более одной на получателя; медленный получатель не задерживает остальные. Настройка групп и forum topics относится к #12, политики важности и тихих часов к #13.

### Гарантии и сроки

Доставка имеет семантику at-least-once в пределах сроков и лимитов: потерянное подтверждение HTTP или остановка после отправки могут дать повтор. Повтор содержит тот же текст и `ID: d-…`; для события сохраняется также `event_id`. Exactly-once не гарантируется. Недоступные или просроченные доставки переходят в `dead_letter`.

- Событие ожидает отправку до 24 часов после сохранения. Ответ команды ожидает до 10 минут; сохранённый `/status` содержит исходное время снимка.
- Backoff: 5 секунд с удвоением до 15 минут, максимум 128 попыток. `retry_after` Telegram учитывается отдельно; пауза получателя сохраняется даже после истечения исходного сообщения.
- Ошибки HTTP 400/403 при отправке завершают соответствующую доставку. Ошибки токена, webhook и конфликт polling требуют исправления и явного запуска сервиса.
- `pending` и `in_flight` восстанавливаются после остановки. Неоднозначность сохраняется в диагностике. Ошибка записи результата не вызывает немедленной повторной HTTP-отправки: сначала повторяется сохранение результата.

Успешное подтверждение в Telegram отправляется только из outbox. При отказе SQLite или квоте новый update не подтверждается. Telegram хранит updates не более 24 часов; более длительная остановка или backpressure могут потерять ещё не принятые команды. Файл языка сохраняется отдельно и атомарно. Повтор `/language` после остановки до фиксации offset идемпотентен.

### Хранение и диагностика

Состояние находится в `state_dir/garde.sqlite3`, с закрытыми файлами и каталогом 0700. `garde.lock` удерживается единственным writer; daemon диагностики открывает SQLite только для чтения. Не удаляйте lock-файл работающей установки. База привязана к server ID, владельцу и подтверждённой identity Telegram-бота. Замена токена того же бота допустима; перенос базы к другому боту или владельцу автоматически не выполняется.

SQLite использует bundled библиотеку, rollback journal, `synchronous=EXTRA` и cache 2 МиБ. Основная база ограничена 64 МиБ; journal временно требует дополнительного места вплоть до размера базы. При занятых 60 МиБ или менее 16 МиБ свободного места новые данные приостанавливаются. Лимит незавершённых доставок: 10 000. Входное событие ограничено 64 КиБ; сообщение вместе с ID не превышает 4096 UTF-16 единиц.

Завершённые записи хранятся до 30 дней, до 10 000 доставок и в пределах квоты. При давлении квоты старые завершённые записи очищаются небольшими порциями, что отражается в `pruned_records`. Незавершённые доставки не удаляются ради места. Полная история, конфигурируемые лимиты и управление dead letters относятся к #11.

Владелец получает `/delivery` в личном чате. Если Telegram недоступен, выполните на VPS:

```sh
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --delivery-status --config /etc/want-garde/config.json
```

Команда не требует доступа к токену и не обращается к сети. JSON содержит счётчики, время самого старого ожидающего сообщения, число получателей на паузе и последние 20 проблем. Тексты сообщений, chat ID и секреты не выводятся. Exit codes: `0` — нет dead letters или ожидания от 60 секунд; `1` — требуется внимание к очереди; `2` — конфигурация или хранилище недоступны, причина в `error.message_key`. Ошибки SQLite видны также в journal. Проверка только конфигурации через `--check-config` базу не создаёт.

### Ручное обновление и откат

Перед обновлением остановите только `want-garde.service`, сохраните предыдущую ревизию и закрытую копию конфигурации и полного `state_dir`. Копия с остановленным writer включает базу и возможный journal. Храните секреты только на сервере с закрытыми правами. Установка #3 не содержит базы; первый обычный запуск создаёт schema 1, сохраняя владельца, токен и файл языка. История Python не импортируется.

После проверки Linux-артефакта переключите `current`, выполните `--check-config`, запустите сервис и проверьте `/status`, `/delivery` и повторный запуск. Не запускайте два poller с одним токеном. Неизвестная версия схемы и повреждённая база дают явную ошибку без пересоздания данных. Для отката к #3 остановите сервис, верните прежние бинарники и сохраните новую базу: #3 её не читает и не доставляет накопленную очередь. Не восстанавливайте старую копию поверх новых данных автоматически. Зашифрованные резервные копии и автоматический rollback реализуются отдельно.

## English

The daemon persists a normalized event and its deliveries in one SQLite transaction before transmission. A Telegram reply and its next polling offset are also committed together. Unsupported or denied updates advance progress without storing their text. The real event source in #4 is service startup; subsequent issues add collectors.

The current recipient is the locally bound owner in a private chat. Each recipient has independent delivery state. Up to four sends run concurrently, with at most one per recipient; a slow recipient does not hold the others. Group and forum-topic configuration belongs to #12, severity policies and quiet hours to #13.

### Guarantees and deadlines

Delivery is at least once within the deadlines and limits. A missing HTTP acknowledgement or shutdown after a send can cause a duplicate. A retry keeps the original text and `ID: d-…`, plus `event_id` for an event. Exactly-once delivery is not guaranteed. Unrecoverable or expired deliveries become `dead_letter`.

- Events wait up to 24 hours after persistence; command replies up to 10 minutes. A queued `/status` retains the original snapshot time.
- Backoff starts at 5 seconds and doubles to 15 minutes, with at most 128 attempts. Telegram `retry_after` is respected separately; recipient cooldown survives expiration of the original message.
- HTTP 400/403 on send terminate that delivery. Token, webhook, and polling-conflict errors require correction and an explicit service start.
- Pending and interrupted attempts survive shutdown. Diagnostics retain uncertainty. Failure to persist a send result retries that persistence before another HTTP send.

Successful Telegram replies always pass through the outbox. SQLite failures or quotas prevent acknowledgement of a new update. Telegram retains updates for at most 24 hours; longer downtime or backpressure can lose commands that have not yet been accepted. Locale remains in its separate atomically written file. Repeating `/language` after interruption before offset commit is idempotent.

### Storage and diagnostics

State lives in the private `state_dir/garde.sqlite3`, inside a 0700 directory. One writer holds `garde.lock`; daemon diagnostics open SQLite read only. Never remove a running installation's lock file. The database binds server ID, owner, and confirmed Telegram bot identity. Rotating the same bot's token is supported; automatic transfer to another bot or owner is not.

SQLite uses the bundled library, rollback journal, `synchronous=EXTRA`, and a 2 MiB cache. The main database is capped at 64 MiB; its journal can temporarily need up to another database-sized allocation. Admission pauses at 60 MiB used or below 16 MiB free filesystem space. At most 10,000 deliveries remain unfinished. An input event is capped at 64 KiB; a message including its IDs stays within 4096 UTF-16 units.

Completed records are retained for up to 30 days, up to 10,000 deliveries, and within the quota. Quota pressure removes older completed records in small batches and increments `pruned_records`. Unfinished deliveries are preserved. Full history, configurable limits, and dead-letter management belong to #11.

Use private `/delivery` as the owner. When Telegram is unavailable, run the local command above as the service user. It needs no token access and makes no network requests. JSON contains counts, the oldest pending time, the number of recipients on cooldown, and the latest 20 problems. Message contents, chat IDs, and secrets are excluded. Exit codes: `0` for no dead letters or waits of 60 seconds or more; `1` for queue attention; `2` for configuration/storage failure with `error.message_key`. SQLite errors also appear in the journal. `--check-config` does not create the database.

### Manual update and rollback

Stop only `want-garde.service` before an update. Preserve the previous revision and a private server-side copy of configuration and the complete `state_dir`. With the writer stopped, retain the database and any journal together. Keep secrets under restricted permissions. A #3 installation has no database: normal startup creates schema 1 while retaining owner, token, and locale file. Python history is not imported.

Verify the Linux artifact, switch `current`, run `--check-config`, start the service, and check `/status`, `/delivery`, and restart. Never run two pollers with one token. Unsupported schema versions and corrupt databases fail explicitly without replacing data. To return to #3, stop the service, restore the previous binaries, and preserve the new database: #3 neither reads it nor delivers its queue. Never automatically restore an old backup over newer data. Encrypted backup and automatic rollback belong to later issues.
