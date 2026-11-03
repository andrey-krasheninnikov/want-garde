# x-ui/3x-ui observation / Наблюдение x-ui/3x-ui

## Русский

Want Garde читает локальную SQLite панели, journal её systemd unit и выбранные журналы access/3xipl. Он сообщает об авторизации, ошибках входа, изменениях конфигурации, активности и соединениях клиентов. Удалённого управления панелью нет.

### Включение

Монитор по умолчанию выключен. Сохраните прежнюю конфигурацию для отката, установите `want-garde-xui.socket` и `want-garde-xui@.service` из того же пакета, что и бинарники. Используется существующий root-owned `/etc/want-garde-helper/daemon-uid` с UID непривилегированного daemon.

Создайте `/etc/want-garde-helper/xui.json` по [примеру политики](../examples/xui-read-policy.json), owner root, mode 0600. Каталог должен принадлежать root и запрещать запись группе и остальным. Укажите реальный путь базы, точный unit панели и только нужные журналы. Максимум четыре файла. Отсутствующий выбранный журнал даёт диагностику; удалите неиспользуемый источник из политики. База, её каталоги и файлы журналов должны принадлежать root без записи группе и остальным. Каталоги журналов принадлежат root; системная групповая запись, как у `/var/log`, допускается, запись остальным запрещена. Reader открывает компоненты через directory descriptors с `NOFOLLOW` и проверяет владельца конечного файла после открытия. Symlinks не принимаются.

Добавьте в закрытую конфигурацию Want Garde:

```json
{
  "xui": {
    "enabled": true,
    "interval_seconds": 30,
    "client_disconnect_after_seconds": 180,
    "helper_socket": "/run/want-garde-xui.sock"
  }
}
```

Это фрагмент для существующей конфигурации. Интервал снимков: 10..3600 секунд; timeout: 30..86400 секунд. Журналы опрашиваются раз в секунду. Затем:

```sh
systemctl daemon-reload
systemctl enable --now want-garde-xui.socket
systemctl restart want-garde
```

Права базы и журналов панели не меняются. Daemon не получает root, доступ к сырой базе или произвольным root-командам. Helper проверяет UID socket peer и принимает только снимок либо чтение журналов с checkpoint. SQL и пути в запросе запрещены. Сеть helper изолирована; секреты и состояние Want Garde скрыты.

### Просмотр и события

`/xui [PAGE]` показывает состояние источников и клиентов по восемь строк. Команда доступна владельцу в личном чате. `/language` сохраняет язык; новые сообщения используют общий каталог 12 языков и изоляцию динамических значений для ar/he.

```sh
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --xui-status --config /etc/want-garde/config.json
```

Локальная диагностика читает сохранённое состояние без токена, обращения к helper или записи в базу. Exit codes: 0, если монитор выключен либо источники исправны; 1, если состояние устарело или покрытие неполно; 2 при ошибке конфигурации/хранилища.

Первый полный снимок задаёт исходное состояние без исторических connect/disconnect. Журналы при первом запуске начинают с конца. Следующие запуски продолжают сохранённые позиции. Rotation, truncation и потеря journal cursor дают явные пропуски; доступные записи восстанавливаются. Исторический счётчик пропусков не сбрасывается при восстановлении доступности.

Клиент считается online при включённом состоянии и свежем `last_online`. Поддерживаются секунды и миллисекунды. Отсутствующее или некорректное время даёт unknown. Отключение клиента в панели отличается от timeout; удалённый клиент исчезает из списка с событием изменения панели. Сбой или неполный снимок не означает отключение всех клиентов.

Нормализованная схема с `clients`/`client_inbounds` и схема с клиентами в `inbounds.settings` определяются по таблицам/столбцам. Без `client_traffics.last_online` состояние соединений недоступно; остальные наблюдения продолжаются. Это поддержка проверенных форматов, а не гарантия любой версии или fork панели.

### Приватность и ограничения

Конфигурационные значения сохраняются только как полные fingerprints SHA-256. Уведомления содержат идентификаторы объектов и имена изменённых полей. Пароли, UUID авторизации, subscription tokens, приватные ключи, JSON настроек и сырые строки журналов не сохраняются и не отправляются. Метки клиентов и сетевые endpoints из распознанных access records видны владельцу; они входят в историю событий.

База открывается read-only, с `query_only`, отключённым trusted schema и короткой транзакцией. Для работающей панели не используется `immutable=1`. В режиме WAL уже существующие sidecars должны быть доступны для чтения; helper не создаёт и не чинит файлы панели. Блокировка базы, неизвестная схема, повреждение и ошибки прав дают диагностику. Остальные мониторы продолжают работу.

Ограничения чтения: база до 256 MiB, до 1024 строк на таблицу, поле до 1 MiB, fingerprints панели до 512 KiB, список клиентов до 256 KiB; сохранённое состояние и ответ helper до 1 MiB. За опрос читается до 64 KiB каждого файла и до 64 journal records; длинные/повреждённые записи дают пропуск либо диагностику. Непрочитанный backlog виден в диагностике и уменьшается следующими опросами. Лимит не подменяет полный снимок усечённым: прежнее достоверное состояние сохраняется. Незавершённый хвост rotated файла пропускается с явным gap, чтобы не остановить новый журнал.

Helper ограничен четырьмя соединениями, 64 MiB памяти и шестью секундами на процесс. События, состояние, checkpoint и доставка сохраняются атомарно. Повтор log position не создаёт новую доставку; конфликтующие данные с тем же ID отклоняются. At-least-once доставка Telegram допускает идентифицируемые дубли после неоднозначного ответа.

Для отката остановите Want Garde, верните предыдущие бинарники и совместимую прежнюю конфигурацию. Сохраните текущую базу доставок; не заменяйте её старым backup. Отключите новый socket, если предыдущая версия им не пользуется. Панель не перезапускается и не изменяется.

## English

Want Garde reads the local panel SQLite database, its systemd journal, and selected access/3xipl files. Events cover panel authentication, failed logins, configuration changes, client activity, and connectivity. Remote panel administration is excluded.

### Enable

Monitoring defaults to disabled. Keep the previous configuration for rollback. Install `want-garde-xui.socket` and `want-garde-xui@.service` from the same package as the binaries. Reuse the root-owned `/etc/want-garde-helper/daemon-uid` containing the unprivileged daemon UID.

Create `/etc/want-garde-helper/xui.json` from the [policy example](../examples/xui-read-policy.json), owned by root with mode 0600 in a root-owned directory without group/other writes. Select the real database, exact panel unit, and only the required logs, up to four files. Missing selected logs produce diagnostics; remove unused entries. The database, its directories, and log files must be root-owned without group/other writes. Log directories must be root-owned without other writes; system group writes, as on `/var/log`, are allowed. The reader opens components through directory descriptors with `NOFOLLOW` and checks final-file ownership after opening. Symlinks are rejected.

Add the JSON fragment above to the existing private Want Garde configuration. Snapshot interval: 10..3600 seconds; disconnect timeout: 30..86400 seconds. Logs are polled every second. Run the systemd commands above to enable the socket and restart Want Garde.

Panel file permissions stay unchanged. The daemon gains neither root, raw database access, nor arbitrary privileged commands. The helper authenticates the socket peer UID and accepts only snapshot/log-checkpoint requests. Caller SQL and paths are forbidden. Its network is isolated; Want Garde secrets and state are hidden.

### Observe and diagnose

`/xui [PAGE]` shows source health and eight clients per page, only to the owner in private chat. `/language` remains persistent; messages use all 12 locales with dynamic-value isolation for ar/he.

The local diagnostic command above reads saved state without a token, helper requests, or database writes. Exit codes: 0 for disabled or healthy monitoring; 1 for stale/incomplete coverage; 2 for configuration/storage errors.

The first complete snapshot establishes state without historical connect/disconnect alerts. Logs initially start at the tail and later resume checkpoints. Rotation, truncation, and lost journal cursors expose gaps while available records are recovered. Historical gap counts survive recovery.

Online requires an enabled client and recent `last_online`; seconds and milliseconds are supported. Missing/invalid timestamps yield unknown. A disabled client differs from timeout; deleted clients leave the inventory with a panel-change event. Failed or incomplete snapshots never imply that all clients disconnected.

Table/column detection supports normalized `clients`/`client_inbounds` and embedded `inbounds.settings` formats. Without `client_traffics.last_online`, connectivity is unavailable while other observations continue. Support covers verified formats, not every panel version or fork.

### Privacy and limits

Configuration values are stored only as full SHA-256 fingerprints. Notifications identify objects and changed field names. Passwords, authorization UUIDs, subscription tokens, private keys, configuration JSON, and raw log lines are neither persisted nor sent. Recognized client labels and network endpoints are visible to the owner and stored in event history.

The reader uses read-only SQLite, `query_only`, disabled trusted schema, and a short transaction. It does not use `immutable=1` against a running panel. WAL requires readable existing sidecars; the helper never creates or repairs panel files. Locks, unsupported schemas, corruption, and access failures produce diagnostics while other monitors continue.

Read limits: database 256 MiB, 1024 rows per table, 1 MiB per field, 512 KiB panel fingerprints, 256 KiB client inventory, 1 MiB saved state/helper response. Each poll reads up to 64 KiB per file and 64 journal records. Long/damaged records produce a gap or diagnostic. Unread backlog remains visible and drains across later polls. A limit preserves the previous reliable state instead of treating truncation as a complete snapshot. An unterminated rotated tail is skipped with an explicit gap so the new file can progress.

The helper permits four connections, 64 MiB memory, and six seconds per process. Events, state, checkpoints, and delivery intent commit atomically. Replaying a log position does not enqueue another notification; conflicting payloads sharing an ID are rejected. Telegram delivery remains at least once, with identifiable duplicates possible after ambiguous replies.

Rollback stops Want Garde and restores previous binaries and their compatible configuration. Retain the current delivery database; never overwrite it with the old backup. Disable the new socket if the previous version does not use it. The panel is neither restarted nor modified.
