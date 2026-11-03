# Host health / Состояние хоста

## Русский

`/status` доступен владельцу в личном чате. Снимок содержит время RFC 3339, CPU в процентах, число логических ядер, load за 1/5/15 минут и на ядро, RAM, место и inode выбранных файловых систем, состояние reboot-required. Объём памяти и диска показывается в MiB, inode в штуках. Неизвестное измерение содержит причину; оно не равно нулю.

Конфигурация `host_health` необязательна. Старые конфигурации используют следующие значения:

```json
{
  "host_health": {
    "enabled": true,
    "interval_seconds": 60,
    "disk_paths": ["/"],
    "disk_warn_percent": 85,
    "disk_critical_percent": 95,
    "inode_warn_percent": 85,
    "inode_critical_percent": 95,
    "cpu_warn_percent": 90,
    "memory_warn_percent": 90,
    "load_warn_per_core": 2,
    "check_reboot_required": true
  }
}
```

Интервал допускает 10..3600 секунд. Список содержит 1..8 уникальных абсолютных путей до 512 байт, без управляющих символов и `..`. Путь может быть каталогом внутри файловой системы. Это явный список, автоматического обнаружения mounts нет. Если `/` исключён, совместимое поле `root_disk` недоступно; список `disks` остаётся источником выбранных измерений.

Проценты допускают 5..100, warning меньше critical. Load допускает 0.2..100 на ядро. RAM использует `MemAvailable`, CPU разность счётчиков за окно 200 ms. Пороги диска используют занятые блоки относительно всех блоков, как Python-прототип; доступное пользователю место показывается отдельно и учитывает резерв файловой системы. Inode без достоверного общего числа недоступны.

CPU, RAM и load требуют двух последовательных превышений. Диски, inode и reboot применяются сразу. В неизменном состоянии уведомление не повторяется. Восстановление использует гистерезис 5 процентных пунктов или 0.2 load на ядро: например, CPU warning при 90%, восстановление при 85%. Critical диска снимается при 90%, warning при 80%. Переход critical в warning не означает полное восстановление.

Активные тревоги сохраняются после рестарта. Неподтверждённая серия замеров сбрасывается после рестарта, неизвестного значения, обратного хода времени или промежутка более двух интервалов. Неизвестное значение сохраняет активную тревогу; доступность источника имеет отдельные события. При отказе записи процесс удерживает одно наблюдение и повторяет его сохранение с паузой 5 секунд. Данные и намерения доставки фиксируются атомарно до отправки.

Для файловых систем нужен GNU coreutils `/usr/bin/stat`. Он запускается без shell и секретов окружения, с timeout 1 секунда и выводом до 4 KiB. На путь допускается только одна незавершённая проба. Проба работает под UID службы; отказ прав виден как недоступность. Не ослабляйте systemd hardening для доступа к закрытым каталогам. Ограниченные процессы не позволяют медленному mount удерживать event loop; состояние D в ядре может задержать фактическое завершение процесса до восстановления самого mount.

На Debian/Ubuntu проверяется `/run/reboot-required`. Отсутствие файла означает «не требуется» только при наличии `/usr/share/update-notifier/notify-reboot-required`. На другой ОС или без надёжного источника результат неизвестен. Проверка не устанавливает пакеты и не выполняет перезагрузку.

Локальная диагностика не требует Telegram token и не изменяет SQLite:

```sh
sudo -u want-garde /usr/local/lib/want-garde/current/want-garde-daemon --host-status --config /etc/want-garde/config.json
```

JSON содержит последний снимок, активные тревоги, ошибки и время проверки. Данные старше двух интервалов считаются устаревшими. Exit codes: 0 для нормы, 1 для тревоги, неполного покрытия, отключённого monitor или устаревания, 2 для ошибки диагностики. `--check-config` проверяет настройки; права на конкретные источники проверяются при сборе.

Подтверждённая среда предыдущей установки: Ubuntu 24.04 x86_64. Приёмка новых проверок фиксируется отдельно в #7. Сборка macOS и синтетические тесты не доказывают поддержку других Linux/ARM64; общая матрица относится к #32.

## English

`/status` is available to the owner in a private chat. It reports an RFC 3339 timestamp, CPU percentage, logical core count, 1/5/15-minute and per-core load, RAM, configured filesystem space and inode usage, and reboot-required status. Memory and disk sizes use MiB; inode values are counts. Unknown measurements carry a reason and never become zero.

The optional `host_health` configuration defaults to the JSON above. Intervals range from 10 to 3600 seconds. Configure 1 to 8 unique absolute paths, each at most 512 bytes, without control characters or `..`. A path may identify a directory inside a filesystem. Mount discovery is explicit. If `/` is excluded, the compatibility field `root_disk` is unavailable; use the `disks` list for configured measurements.

Percentage thresholds range from 5 to 100; warning must be below critical. Load thresholds range from 0.2 to 100 per core. RAM uses `MemAvailable`; CPU uses a 200 ms counter delta. Disk thresholds use occupied blocks divided by all blocks, preserving the prototype calculation. User-available space is reported separately and reflects filesystem reservation. Inodes without a reliable total remain unavailable.

CPU, memory, and load require two consecutive excessive samples. Disk, inode, and reboot transitions apply immediately. An unchanged state produces no repeat. Recovery uses 5 percentage points of hysteresis, or 0.2 load per core. CPU warns at 90% and recovers at 85%. Disk critical clears at 90%, warning at 80%; a critical-to-warning transition is only partial recovery.

Active alerts survive restart. Incomplete confirmation resets after restart, an unknown measurement, backward clock movement, or a gap exceeding two intervals. Unknown values retain active alerts. Source availability has separate events. A failed write retains one observation and retries persistence every 5 seconds. Snapshot, states, events, and delivery intent commit atomically before sending.

Filesystem collection requires GNU coreutils at `/usr/bin/stat`. It runs without a shell or inherited secrets, with a 1-second timeout and 4 KiB output limit. Only one unfinished probe is allowed per path. The service UID performs each probe; denied access is reported. Preserve systemd hardening when a directory is inaccessible. Probes do not block the event loop, although a kernel D-state process may finish only after its mount recovers.

Debian/Ubuntu use `/run/reboot-required`. A missing marker means no reboot is required only when `/usr/share/update-notifier/notify-reboot-required` exists. Other operating systems or missing reliable sources report unknown. The monitor installs no packages and performs no reboot.

The command above reads local diagnostics without the Telegram token or SQLite writes. JSON contains the latest snapshot, alerts, errors, and check time. Data older than two intervals is stale. Exit codes are 0 for normal, 1 for alerts, unavailable data, disabled monitoring, or stale data, and 2 for diagnostic failure. `--check-config` validates settings; runtime collection checks source access.

The previous installation was verified on Ubuntu 24.04 x86_64. New acceptance evidence is recorded separately in #7. macOS builds and synthetic tests do not establish other Linux/ARM64 support; #32 owns the complete matrix.
