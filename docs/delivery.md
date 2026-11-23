# Delivery and recovery / Доставка и восстановление

## Русский

Daemon сохраняет нормализованное событие и все его доставки одной транзакцией SQLite. Ответ на команду и следующий polling offset сохраняются вместе. Запрещённая или неподдерживаемая команда обновляет offset без сохранения текста.

### Получатели уведомлений

По умолчанию события получает локально привязанный владелец в личном чате. Поле `notifications` в `config.json` позволяет добавить личные чаты и форумы.

```json
{
  "notifications": {
    "private": [
      {
        "chat_id": "234567890",
        "options": { "locale": "ru", "protect_content": true }
      }
    ],
    "forums": [
      {
        "chat_id": "-1001234567890",
        "default_topic_id": 20,
        "topics": [{ "event_type": "ssh.failed", "topic_id": 42 }],
        "options": { "locale": "en", "silent": true, "privacy": "redacted" }
      }
    ]
  }
}
```

- Укажите положительный строковый `chat_id` для личного чата и отрицательный для форумной супергруппы. Личный получатель сначала открывает чат с ботом. Чтобы настроить форум, используйте `message.chat.id` и `message.message_thread_id` из сообщения в нужной теме. Боту требуется право отправки сообщений.
- Каждый форум требует явный `default_topic_id`. Укажите ID темы больше 1 или `null` для General. `event_type` совпадает с `classification.rule_id` события. Например, `ssh.failed` направляется в тему 42, а `ssh.login` в тему 20. Для каждого типа в форуме задаётся один маршрут. `topic_id: null` явно направляет конкретный тип в General.
- В одной установке до четырёх получателей с учётом владельца. Каждый чат указывается один раз. `retention.pending_deliveries` должен вмещать 2048 доставок на получателя, чтобы обработать максимальный пакет событий одной транзакцией.
- `silent` и `protect_content` по умолчанию выключены и задаются отдельно. `locale` выбирает один из 12 языков. Дополнительный получатель без `locale` использует `default_locale`. Язык владельца берётся из `/language`, если его не переопределяет `notifications.owner.locale`.
- Режим `full` показывает полный текст уведомления, включая GeoIP при наличии контекста. Это режим по умолчанию для владельца. Остальные получатели по умолчанию используют `redacted`. Он скрывает имя пользователя, заменяет последний октет IPv4 на `x`, скрывает IPv6 целиком и опускает остальные подтверждающие данные, GeoIP и подсказку `/geoip`. Для владельца параметры уведомлений задаются в `notifications.owner`.

Команды доступны только владельцу в личном чате. Входящее сообщение не добавляет получателя. Конфигурация применяется к новым уведомлениям. Доставка сохраняет чат, тему, параметры отправки и режим приватности при постановке в очередь, включая повторы и перезапуск.

Daemon проверяет режим форума и отправляет сообщение отдельными запросами к Telegram. Если отключить форум между запросами, Telegram может отправить сообщение в чат без темы. Оставляйте режим форума включённым, пока чат используется для доставки. При отказе Telegram доставка завершается без перехода в General или другую тему. Личные доставки выполняются независимо. В диагностике `telegram.forum_required` означает, что чат не форум, `telegram.forum_topic` означает отклонённый запрос к чату или теме, `telegram.forum_forbidden` означает запрет доступа, а `telegram.forum_response` отмечает ответ с неверной темой или неполными данными проверки чата. [Telegram Bot API](https://core.telegram.org/bots/api#sendmessage)

### Гарантии и сроки

Доставка имеет семантику at-least-once в пределах сроков и лимитов: потерянное подтверждение HTTP или остановка после отправки могут дать повтор. Повтор содержит тот же текст и `ID: d-…`; для события сохраняется также `event_id`. Exactly-once не гарантируется. Недоступные или просроченные доставки переходят в `dead_letter`.

- Событие ожидает отправку до 24 часов после сохранения. Ответ команды ожидает до 10 минут; сохранённый `/status` содержит исходное время снимка.
- Backoff: 5 секунд с удвоением до 15 минут, максимум 128 попыток. `retry_after` Telegram учитывается отдельно; пауза получателя сохраняется даже после истечения исходного сообщения.
- Ошибки HTTP 400/403 при отправке завершают соответствующую доставку. Ошибки токена, webhook и конфликт polling требуют исправления и явного запуска сервиса.
- `pending` и `in_flight` восстанавливаются после остановки. Неоднозначность сохраняется в диагностике. Ошибка записи результата не вызывает немедленной повторной HTTP-отправки: сначала повторяется сохранение результата.

Успешное подтверждение в Telegram отправляется только из outbox. При отказе SQLite или квоте новый update не подтверждается. Telegram хранит updates не более 24 часов; более длительная остановка или backpressure могут потерять ещё не принятые команды. Файл языка сохраняется отдельно и атомарно. Повтор `/language` после остановки до фиксации offset идемпотентен.

После 48 часов без принятых updates сохранённый polling offset сбрасывается перед следующим запросом. Это больше срока хранения Telegram и раньше возможного изменения последовательности ID после недели простоя. Дедупликация использует новую последовательность; принятые доставки сохраняют свои ID и текст. См. [правила Telegram](https://core.telegram.org/bots/api#update).

### Хранение и диагностика

Состояние находится в `state_dir/garde.sqlite3`, с закрытыми файлами и каталогом 0700. `garde.lock` удерживается единственным writer; daemon диагностики открывает SQLite только для чтения. Не удаляйте lock-файл работающей установки. База привязана к server ID, владельцу и подтверждённой identity Telegram-бота. Замена токена того же бота допустима; перенос базы к другому боту или владельцу автоматически не выполняется.

SQLite использует bundled библиотеку, rollback journal, `synchronous=EXTRA` и cache 2 МиБ. По умолчанию основная база ограничена 64 МиБ; journal временно требует дополнительного места вплоть до размера базы. Приём приостанавливается, когда занято не меньше квоты базы минус 4 МиБ (60 МиБ по умолчанию) или свободного места меньше квоты базы плюс 16 МиБ. По умолчанию очередь вмещает 10 000 незавершённых доставок и одно резервное место для /health. Входное событие ограничено 64 КиБ; сообщение вместе с ID не превышает 4096 UTF-16 единиц.

По умолчанию завершённые записи хранятся до 30 дней, до 10 000 доставок и в пределах квоты. При давлении квоты старые завершённые записи очищаются небольшими порциями, что отражается в `pruned_records`. Незавершённые доставки не удаляются ради места. Настраиваемые лимиты и общая диагностика описаны в [истории и хранении](history.md).

Владелец получает `/delivery` в личном чате. Если Telegram недоступен, выполните на VPS:

```sh
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --delivery-status --config /etc/want-garde/config.json
```

Команда не требует доступа к токену и не обращается к сети. JSON содержит счётчики, время самого старого ожидающего сообщения, число получателей на паузе и последние 20 проблем. Тексты сообщений, chat ID и секреты не выводятся. Exit codes: `0` — нет dead letters или ожидания от 60 секунд; `1` — требуется внимание к очереди; `2` — конфигурация или хранилище недоступны, причина в `error.message_key`. Ошибки SQLite видны также в journal. Проверка только конфигурации через `--check-config` базу не создаёт.

### Ручное обновление и откат

Перед обновлением остановите только `want-garde.service`, сохраните предыдущую ревизию и закрытую копию конфигурации и полного `state_dir`. Копия с остановленным writer включает базу и возможный journal. Храните секреты только на сервере с закрытыми правами. Запуск создаёт schema 3 или обновляет schema 1 или 2 одной транзакцией. Существующие доставки сохраняют ID, чат, тему и текст. Владелец, токен и файл языка сохраняются.

После проверки Linux-артефакта переключите `current`, выполните `--check-config`, запустите сервис и проверьте `/status`, `/delivery` и повторный запуск. Не запускайте два poller с одним токеном. Неизвестная версия схемы и повреждённая база дают явную ошибку без пересоздания данных. Бинарники со schema 1 или 2 отклоняют базу schema 3 до отправки защищённых ответов. Для отката сохраните текущую базу отдельно и используйте совместимую копию `state_dir` и конфигурации, сделанную до обновления. Проверьте состояние доставки перед повторной отправкой из резервной копии. Зашифрованные резервные копии и автоматический rollback реализуются отдельно.

## English

The daemon commits a normalized event and all its deliveries in one SQLite transaction. It commits each command reply with the next polling offset. A denied or unsupported update advances the offset without storing the command text.

### Notification recipients

Events go to the locally bound owner in a private chat by default. Add private chats and forums under `notifications` in `config.json`, as shown above.

- Use a positive quoted `chat_id` for a private chat and a negative quoted ID for a forum supergroup. A private recipient opens the chat with the bot first. For a forum, use `message.chat.id` and `message.message_thread_id` from a message in the intended topic. Give the bot permission to send messages.
- Each forum requires an explicit `default_topic_id`. Use a topic ID greater than 1 or `null` for General. An `event_type` matches the event's `classification.rule_id`. In the example, `ssh.failed` goes to topic 42 and `ssh.login` goes to topic 20. Each event type can have one route per forum. Set `topic_id: null` to route a specific event type to General.
- One installation supports up to four recipients including the owner. Each chat can appear once. Set `retention.pending_deliveries` to at least 2048 deliveries per recipient so the largest event batch fits in one transaction.
- Set `silent` and `protect_content` independently. Both default to off. Set `locale` to one of the 12 supported languages. Additional recipients inherit `default_locale` when omitted. The owner uses `/language` unless `notifications.owner.locale` overrides it.
- `full` shows the full notification, including GeoIP when available. This is the owner default. Other recipients default to `redacted`, which hides the username, replaces the last IPv4 octet with `x`, and hides the entire IPv6 address. It omits other evidence, GeoIP, and the `/geoip` hint. Use `notifications.owner` to set the owner's notification options.

Commands require the owner's private chat. Incoming messages grant no subscription. Configuration changes apply to new deliveries. A queued delivery uses the same chat, topic, send options and privacy mode through retries and restarts.

The sender verifies forum mode and posts in separate Telegram requests. If forum mode is disabled between those requests, Telegram can post the message to the chat without a topic. Keep forum mode enabled while the chat is configured for delivery. If Telegram rejects a topic request, the delivery ends without a fallback. Private deliveries continue independently. In diagnostics, `telegram.forum_required` means the chat is not a forum, `telegram.forum_topic` means Telegram rejected the chat or topic request, `telegram.forum_forbidden` marks a permission failure, and `telegram.forum_response` marks a topic mismatch or incomplete chat verification. [Telegram Bot API](https://core.telegram.org/bots/api#sendmessage)

### Guarantees and deadlines

Delivery is at least once within the deadlines and limits. A missing HTTP acknowledgement or shutdown after a send can cause a duplicate. A retry keeps the original text and `ID: d-…`, plus `event_id` for an event. Exactly-once delivery is not guaranteed. Unrecoverable or expired deliveries become `dead_letter`.

- Events wait up to 24 hours after persistence; command replies up to 10 minutes. A queued `/status` retains the original snapshot time.
- Backoff starts at 5 seconds and doubles to 15 minutes, with at most 128 attempts. Telegram `retry_after` is respected separately; recipient cooldown survives expiration of the original message.
- HTTP 400/403 on send terminate that delivery. Token, webhook, and polling-conflict errors require correction and an explicit service start.
- Pending and interrupted attempts survive shutdown. Diagnostics retain uncertainty. Failure to persist a send result retries that persistence before another HTTP send.

Successful Telegram replies always pass through the outbox. SQLite failures or quotas prevent acknowledgement of a new update. Telegram retains updates for at most 24 hours; longer downtime or backpressure can lose commands that have not yet been accepted. Locale remains in its separate atomically written file. Repeating `/language` after interruption before offset commit is idempotent.

After 48 hours without accepted updates, the saved polling offset resets before the next request. This exceeds Telegram retention and precedes its possible ID-sequence change after an idle week. Deduplication uses a new sequence; accepted deliveries retain their IDs and text. See [Telegram's rules](https://core.telegram.org/bots/api#update).

### Storage and diagnostics

State lives in the private `state_dir/garde.sqlite3`, inside a 0700 directory. One writer holds `garde.lock`; daemon diagnostics open SQLite read only. Never remove a running installation's lock file. The database binds server ID, owner, and confirmed Telegram bot identity. Rotating the same bot's token is supported; automatic transfer to another bot or owner is not.

SQLite uses the bundled library, rollback journal, `synchronous=EXTRA`, and a 2 MiB cache. By default, the main database is capped at 64 MiB. Its journal can temporarily need up to another database-sized allocation. Admission pauses when occupied space reaches the database limit minus 4 MiB (60 MiB by default), or free space falls below the database limit plus 16 MiB. The default queue limit is 10,000 unfinished deliveries plus one reserved slot for /health. An input event is capped at 64 KiB; a message including its IDs stays within 4096 UTF-16 units.

By default, completed records are retained for up to 30 days, up to 10,000 deliveries, and within the quota. Quota pressure removes older completed records in small batches and increments `pruned_records`. Unfinished deliveries are preserved. See [history and retention](history.md) for configurable limits and installation health.

Use private `/delivery` as the owner. When Telegram is unavailable, run the local command above as the service user. It needs no token access and makes no network requests. JSON contains counts, the oldest pending time, the number of recipients on cooldown, and the latest 20 problems. Message contents, chat IDs, and secrets are excluded. Exit codes: `0` for no dead letters or waits of 60 seconds or more; `1` for queue attention; `2` for configuration/storage failure with `error.message_key`. SQLite errors also appear in the journal. `--check-config` does not create the database.

### Manual update and rollback

Stop only `want-garde.service` before an update. Preserve the previous revision and a private server-side copy of configuration and the complete `state_dir`. With the writer stopped, retain the database and any journal together. Keep secrets under restricted permissions. Startup creates schema 3 or upgrades schema 1 or 2 in one transaction. Existing deliveries keep their IDs, chats, topics, and text. The owner, token and locale file are preserved.

Verify the Linux artifact, switch `current`, run `--check-config`, start the service, and check `/status`, `/delivery`, and restart. Never run two pollers with one token. Unsupported schema versions and corrupt databases fail explicitly without replacing data. Schema 1 and 2 binaries reject a schema 3 database before sending protected replies. To roll back, preserve the current database separately and use the compatible `state_dir` and configuration backup made before the upgrade. Check delivery status before resending from a backup. Encrypted backup and automatic rollback belong to later issues.
