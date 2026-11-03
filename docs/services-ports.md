# Services and listening ports / Службы и listening ports

## Русский

Мониторинг служб читает только явный список systemd units. Пустой список отключает этот источник.
Пример секций конфигурации:

```json
{
  "services": { "units": ["ssh.service", "fail2ban.service"], "interval_seconds": 60 },
  "ports": {
    "enabled": true,
    "interval_seconds": 120,
    "ignore_patterns": [],
    "ignore_ephemeral_udp": true,
    "ephemeral_range": null,
    "important_udp_ports": [53, 67, 68, 123, 323, 443, 500, 4500, 51820],
    "expected_process_names": []
  }
}
```

Выберите точные установленные `.service` имена, без glob или путей, максимум 32.
Интервалы допускают от 10 до 3600 секунд. `inactive`/`failed` дают critical сразу, включая первый запуск.
`active` после падения даёт recovered; `active/exited` работает как active.
Переходные состояния не дают ложного падения или восстановления.
Неизвестная unit, ошибка доступа, timeout или неизвестный формат показывают потерю покрытия.
Наблюдение дискретное: изменения между проверками могут не попасть в снимки.

`/services [PAGE]` и `/ports [PAGE]` доступны только владельцу в личном чате.
Страница содержит до восьми строк. Текстовые значения ограничены 128 символами;
полные доступные данные остаются в локальном JSON. Группы, чужие пользователи,
callbacks и edited messages не получают сведения. Команды читают сохранённый снимок;
после двух интервалов или при обратном ходе времени он считается устаревшим.

Первый полный снимок endpoints создаёт baseline с отдельным событием.
Неполные сведения о процессах не мешают сохранению полного набора endpoints.
Baseline не меняется при новых наблюдениях; последний снимок хранится отдельно.
Новый endpoint вне baseline даёт critical, исчезновение и возврат известного endpoint дают info.
Неизменный drift не повторяет события, но остаётся виден в диагностике.
Сохраняются baseline, состояния, события и очередь, включая перезапуск без Telegram.
Неполный снимок не создаёт массовое исчезновение портов и не заменяет baseline.

Фильтр исключает ephemeral UDP, кроме important ports. Диапазон берётся из
`/proc/sys/net/ipv4/ip_local_port_range`; недоступность включает fallback 32768..60999
и видимое ограничение покрытия. Явный `ephemeral_range` имеет вид `[32768, 60999]`.
`ignore_patterns` сопоставляет строки вида `tcp 127.0.0.1:8080`, поддерживает `*`, `?`,
классы и диапазоны `[abc]`, `[0-9]`, `[!0-9]`. Лимит: 32 ASCII patterns до 128 bytes.
Изменение фильтра требует нового явного принятия, прежний baseline не стирается.

Имена bash/dash/sh, nc/ncat/netcat/socat, node/perl/php/python/ruby дают warning.
Это объяснимая эвристика по имени, а не доказательство атаки.
Она действует и для исключённых портов, и после принятия baseline.
`expected_process_names` исключает ожидаемые имена из этой эвристики (до 64 имён).
PID не входит в baseline; аргументы и environment не читаются и не сохраняются.
Читается только network namespace хоста, без перебора namespaces контейнеров.

### Listener read helper

Отдельный socket `/run/want-garde-listeners.sock` принимает только `ports.status`.
Helper проверяет kernel peer UID из существующей root-owned policy
`/etc/want-garde-helper/daemon-uid`. Подготовьте policy по [инструкции helper](firewall.md).
Существующие fail2ban units и их `PrivateNetwork=true` сохраняются.
Новый helper видит сеть хоста, имеет только `CAP_DAC_READ_SEARCH` и `CAP_SYS_PTRACE`,
разрешает AF_UNIX/AF_NETLINK и запрещает process-control syscalls.
Файлы конфигурации, состояния, backup и секреты закрыты для него.
Daemon не получает root, capabilities, sudo или Docker socket.

```sh
sudo install -m 0644 installation/want-garde-listeners.socket /etc/systemd/system/
sudo install -m 0644 installation/want-garde-listeners@.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now want-garde-listeners.socket
sudo systemctl restart want-garde.service
```

Фиксированный `ss` читает отдельно IPv4/IPv6 TCP/UDP listeners без DNS и shell.
Проба ограничена 64 KiB на family; response ограничен 512 KiB, lifetime unit 6 секунд,
одновременно допускаются четыре соединения. До 512 endpoints и четырёх процессов
на endpoint; превышение лимита отражается как неполное покрытие.
При отсутствии helper остаётся непривилегированный сбор с явным ограничением processes.
Некоторые PID могут быть недоступны даже с helper или исчезнуть во время чтения.

### Локальная диагностика и baseline

```sh
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --services-status --config /etc/want-garde/config.json
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --ports-status --config /etc/want-garde/config.json
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --ports-baseline-preview --config /etc/want-garde/config.json
```

Status-команды читают SQLite без token и сети: exit 0 означает норму, 1 требует внимания,
2 означает ошибку конфигурации/хранилища. Preview делает новый локальный сбор,
показывает точные endpoints/diff/hash и не меняет базу. Exit 0 означает готовый preview,
2 означает ошибку. Для применения сначала проверьте JSON, затем остановите daemon:

```sh
sudo systemctl stop want-garde.service
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --ports-baseline-accept HASH --config /etc/want-garde/config.json
sudo systemctl start want-garde.service
```

Замените HASH точным значением preview. Accept повторяет сбор и проверяет hash,
привязанный к серверу, baseline revision, фильтрам и endpoints. Изменившийся preview
или занятый writer lock отклоняется с exit 2. Успех возвращает JSON и exit 0.
Повтор уже использованного hash отклоняется; после неоднозначного ответа сначала
прочитайте `--ports-status`. Не повторяйте изменение вслепую.
Принятие, audit event с локальным UID и намерение доставки сохраняются атомарно;
при ошибке обязательной записи baseline не меняется. Telegram не принимает baseline.
После проверки или исправления ошибки снова запустите daemon.

Перед обновлением сохраните закрытую копию configuration/state при остановленном writer
и прежнюю версию. SQLite schema остаётся 1; версии #7 игнорируют новые metadata.
При откате остановите и отключите только новый listener socket, верните прежние binaries,
сохраните новые данные и backup. Версия #7 отклоняет новые секции `services`/`ports`: перед её запуском удалите эти секции из копии текущей конфигурации либо верните прежнюю конфигурацию после проверки последующих изменений. Не восстанавливайте старую базу поверх новых данных автоматически.

## English

Service monitoring reads an explicit list of up to 32 installed `.service` units; an empty list
turns it off. The configuration above shows the defaults, except the example service selection.
Intervals range from 10 to 3600 seconds. A confirmed `inactive`/`failed` state immediately produces
critical, including first observation. Returning to `active` produces recovery; `active/exited` is active.
Transitional states and unavailable data never clear a known failure. Unknown units, permission
errors, timeouts and unsupported output visibly limit coverage. Changes between polls can be missed.

The owner uses `/services [PAGE]` and `/ports [PAGE]` in a private chat. Groups, strangers,
callbacks and edited messages receive no data. Each page has up to eight rows; displayed values
are bounded to 128 characters. Full available values remain in local JSON. Commands read saved
observations, marked stale after two intervals or a backwards clock change.

The first complete endpoint snapshot initializes a durable baseline with a separate event.
Partial process metadata does not invalidate a complete endpoint set. Later observations never
replace the baseline. Keep the last observation separately: unchanged drift produces no repeated
event, while diagnostics retain the difference. New endpoints outside the baseline are critical;
disappearance and return of known endpoints are info. Persist state/events/outbox atomically,
including offline collection and restart. Incomplete collection never implies mass removal.

Ephemeral UDP is ignored except important ports. Read the range from the proc sysctl above;
when unavailable, visibly fall back to 32768..60999. `ephemeral_range` may explicitly select a range.
Up to 32 ASCII `ignore_patterns` of 128 bytes match protocol/address/port strings using `*`, `?`,
classes, ranges and negated classes. Filter changes require explicit baseline acceptance and preserve
the old baseline. Diagnostics show ignored counts and range fallback.

Shell, netcat/socat and interpreter names listed in the Russian section produce an explained
name-based warning, including ignored endpoints and accepted baselines. This is not proof of an
attack. Up to 64 `expected_process_names` suppress known names. PID changes do not alter the
baseline. Never collect argv or environment. Enumerate the host network namespace only.

Install the separate listener socket/service with the commands above after preparing the existing
root-owned daemon UID policy. Keep fail2ban helper isolation unchanged. The listener helper uses
only a fixed `ss` command, the host network namespace, CAP_DAC_READ_SEARCH/CAP_SYS_PTRACE and
AF_UNIX/AF_NETLINK. Process-control syscalls and installation secrets/state/backups are inaccessible.
The daemon remains unprivileged. Each family read is bounded to 64 KiB, response to 512 KiB,
unit lifetime to six seconds and concurrency to four connections. Support up to 512 endpoints and
four process records per endpoint; limits are visible. Fall back to unprivileged endpoint collection
when the helper is unavailable, explicitly reporting incomplete process coverage. Individual PID
information can still be unavailable or race with collection.

The local status commands above read SQLite without token access or network. Exit codes are 0
for healthy, 1 for attention and 2 for configuration/storage errors. Preview collects fresh local
listeners and reads state without changing SQLite; exit 0 means a valid preview, 2 an error.
Inspect endpoints/diff/hash, stop the daemon, run accept with that exact hash, then restart.
Accept rescans and binds the hash to server, baseline revision, filters and endpoints. Stale hashes
and an occupied writer lock fail with exit 2. Success returns JSON and exit 0. A used hash cannot
be replayed; inspect `--ports-status` after an ambiguous response before any further change.
Acceptance, audit event with local UID and delivery intent commit together; failed audit/storage
never changes the baseline. No Telegram baseline mutation is exposed.

Before rollout, retain a private stopped-writer configuration/state backup and previous binaries.
Schema 1 stays compatible with #7, which ignores the added metadata. Rollback disables only the
new listener socket and restores previous binaries while retaining new data and the backup.
Version #7 rejects the new `services`/`ports` configuration sections: remove them from a copy of the current configuration or restore the previous configuration after checking subsequent edits. Never automatically overwrite newer data with an older database. Linux runtime acceptance is
separate from macOS checks; the remaining distribution/ARM64 matrix belongs to #32.
