# История и хранение / History and retention

## Русский

Владелец в личном Telegram-чате может запросить `/recent [1–10 [CURSOR]]`, `/history EVENT_ID`,
`/stats [1–168]` и `/health`. `/recent` выдаёт до 10 записей в порядке сохранения и показывает
команду для следующей страницы. В истории видны исходное время, источник, важность и ID.
`/stats` считает сохранённые события за последние 24 часа по умолчанию и показывает до 24 групп.
`/health` показывает наблюдение, пропуски, очередь, ошибки доставки и использование хранилища.

Локальные команды `want-garde-daemon --recent`, `--history EVENT_ID`, `--stats` и `--health`
поддерживают те же аргументы перед `--config PATH`. Они читают SQLite без токена. Применяются
права каталога состояния. JSON выдаётся в stdout. Коды 0, 1 и 2 означают результат,
деградацию health и ошибку запроса или чтения соответственно.

Необязательная секция `retention` в конфигурации задаёт `event_days` (30),
`action_audit_days` (30), `process_audit_days` (7), `result_hours` (24),
`database_mib` (64), `pending_deliveries` (10000) и `result_mib` (32).
При нехватке места удаляются доступные для очистки записи и приём приостанавливается.
Ожидающие доставки сохраняются до обычного срока доставки. Ограничение очереди от 2048
сообщений вмещает одну публикацию наблюдений. Для `/health` есть одно дополнительное место.
Локальный `--health` доступен при нехватке места, блокировке более ранней команды
или задержке отправки из-за очереди.
Срок завершённой доставки считают от результата. Событие хранится, пока на него ссылается доставка. Завершённые файлы результатов
хранятся отдельно. Незавершённые операции не используют этот каталог. Перед уменьшением квоты
базы проверьте её физический размер. Запуск с квотой ниже текущего размера завершается ошибкой.

Новые таблицы и индекс добавляются при открытии базы. События, ID и ожидающая очередь
сохраняются. Перед сменой версии сохраните согласованную копию базы и конфигурации.
Версии до #11 открывают только базу физическим размером до 64 МиБ и конфигурацию
без секции `retention`.

## English

The owner can use `/recent [1–10 [CURSOR]]`, `/history EVENT_ID`, `/stats [1–168]`, and `/health`
in a private Telegram chat. Recent history lists up to ten events in storage order and includes
a command for the next page. Entries show the original time, source, severity, and ID.
Stats cover the last 24 hours by default and list up to 24 groups. Health reports source coverage,
gaps, queued deliveries, delivery errors, and storage usage.

Run the equivalent `want-garde-daemon --recent`, `--history EVENT_ID`, `--stats`, or `--health`
with the same arguments before `--config PATH`. These commands read SQLite without the token
using the permissions of the state directory. They write JSON to stdout. Exit 0 means success,
1 means degraded health, and 2 means an invalid request or read failure.

The optional `retention` section configures `event_days` (30), `action_audit_days` (30),
`process_audit_days` (7), `result_hours` (24), `database_mib` (64), `pending_deliveries` (10000),
and `result_mib` (32). Storage pressure prunes eligible records and can pause intake.
Pending deliveries retain their usual deadlines. The 2048-message minimum fits one
observation batch. The queue reserves one additional slot for `/health`.
Use local `--health` if disk space is low, an earlier update is blocked, or the queue delays the reply.
Terminal delivery retention starts at the outcome. Events with delivery references remain available. Completed result files live in a separate directory.
In-progress operations do not use it. Check the physical database size before lowering the quota.
Startup fails if the database already exceeds the configured limit.

The writer adds the new tables and index when opening the database. Existing events, IDs,
and pending deliveries survive the upgrade. Make a consistent copy of the database and configuration
before changing versions. Releases before #11 can open a database up to 64 MiB with settings
without the `retention` section.

Keep a database larger than 64 MiB on a compatible release.
