# ADR-0005: Durable delivery / Сохраняемая доставка

Status / Статус: Accepted / Принято.

## Context / Контекст

Telegram sends and SQLite commits cannot form one transaction. A process can stop after transmission but before recording success. The installation has one writer and a small resource budget; it also needs useful diagnostics when Telegram is unavailable.

Отправка Telegram и фиксация SQLite не образуют общую транзакцию. Процесс может остановиться после отправки, но до записи успеха. Установка имеет один writer и небольшой бюджет ресурсов; диагностика нужна и при недоступном Telegram.

## Decision / Решение

Persist event and delivery intent atomically. Persist a command reply and polling progress atomically. Claim an attempt before HTTP; retain stable message IDs and content through retries. Interrupted attempts become uncertain. Delivery is at least once within deadlines and quotas; permanent errors, expiration, and exhausted retries produce diagnosable dead letters.

Сохранять событие и намерение доставки атомарно. Сохранять ответ команды и polling progress атомарно. Перед HTTP фиксировать попытку; повторы сохраняют ID и текст. Прерванные попытки имеют неоднозначный результат. Семантика at least once ограничена сроками и квотами; постоянная ошибка, истечение срока и исчерпание повторов дают диагностируемые dead letters.

One daemon thread owns the SQLite writer behind a bounded request queue and process-lifetime file lock. Use bundled SQLite with rollback journal and synchronous EXTRA. Local daemon diagnostics open the same database read only without claiming or recovering work. CLI/MCP/TMA do not gain direct database access. Initial schema and installation identity commit together; unknown or incomplete schemas are never rebuilt automatically.

Один поток daemon владеет SQLite writer через ограниченную очередь запросов и file lock на время процесса. Использовать bundled SQLite, rollback journal и synchronous EXTRA. Локальная команда daemon читает базу без claim или восстановления отправок. CLI/MCP/TMA не получают прямого доступа к базе. Начальная схема и identity установки фиксируются вместе; неизвестная или неполная схема автоматически не пересоздаётся.

## Alternatives / Альтернативы

An in-memory queue loses pending messages on shutdown. Marking success before HTTP loses messages after a failed send. Exactly-once cannot be established from Telegram's sendMessage acknowledgement. WAL adds checkpoint and sidecar management without a demonstrated need for sustained concurrent readers in this installation.

Очередь в памяти теряет pending messages при остановке. Запись успеха до HTTP теряет сообщения при неудачной отправке. Подтверждение sendMessage не даёт exactly-once. WAL требует управления checkpoint и sidecar без подтверждённой потребности в постоянных параллельных readers этой установки.

## Consequences / Последствия

Duplicates remain possible and identifiable. The rollback journal needs temporary disk space in addition to the bounded database. Full storage pauses admission rather than discarding unfinished work. Locale remains in the existing atomic file; repeating the current language command is idempotent. Returning to #3 preserves the new database but suspends its deliveries. Future migrations must define backup and rollback compatibility before changing durable data.

Возможны идентифицируемые дубликаты. Rollback journal временно требует места сверх ограниченного размера базы. Заполненное хранилище приостанавливает приём вместо удаления незавершённой работы. Язык остаётся в существующем атомарном файле; повтор текущей команды выбора языка идемпотентен. Откат к #3 сохраняет новую базу, но останавливает её доставки. Следующие миграции должны определить backup и совместимость отката до изменения данных.

Revisit journal mode and budgets after measured collector load or a real concurrent-reader requirement.

Пересмотреть journal mode и бюджеты после измерения нагрузки collectors или появления реального требования к параллельным readers.
