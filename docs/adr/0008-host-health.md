# ADR-0008: Host health transitions / Переходы состояния хоста

Status / Статус: Accepted / Принято.

## Decision / Решение

Host snapshots retain available measurements and explicit failures independently. Monitor configured filesystem paths, defaulting to `/`. Bound filesystem probes by time, output, and process count; do not run them inside the event loop. CPU, memory, and load warnings require two consecutive samples. Disk and inode transitions apply immediately. Hysteresis controls recovery. Unknown measurements do not clear an active warning.

Снимок независимо сохраняет доступные показатели и явные ошибки. Проверять заданные пути файловых систем, по умолчанию `/`. Ограничивать время, вывод и число процессов файловых проб; не выполнять их внутри event loop. Предупреждения CPU, RAM и load требуют двух последовательных замеров. Переходы дисков и inode применяются сразу. Гистерезис определяет восстановление. Неизвестное измерение не снимает активную тревогу.

Persist the latest snapshot, transition states, events, and delivery intent in one SQLite transaction. Retain active states across restart, but restart incomplete consecutive-sample confirmation. Reuse existing storage quotas and delivery semantics. The schema remains version 1; bounded host state occupies installation metadata.

Сохранять последний снимок, состояния переходов, события и намерения доставки одной транзакцией SQLite. Активные состояния переживают рестарт; незавершённое подтверждение последовательных замеров начинается заново. Использовать существующие квоты и семантику доставки. Схема остаётся версии 1; ограниченное состояние хоста хранится в метаданных установки.

Reboot detection reports a known operating-system mechanism or an explicit unavailable result. It never infers that a reboot is unnecessary solely because an unsupported marker is absent. No reboot or operating-system update is performed by this monitor.

Проверка reboot использует известный механизм ОС либо сообщает недоступность. Отсутствие неподдерживаемого маркера не доказывает, что перезагрузка не нужна. Источник наблюдений не выполняет reboot или обновление ОС.

## Acceptance / Приёмка

Test observations through temporary SQLite and a replacement Telegram transport, commands through their replies or denials, and diagnostics through JSON and exit codes. Verify real Linux filesystems, permissions, and systemd separately from local macOS checks. Preserve the installation configuration and data during rollout and retain a rollback version.

Проверять наблюдения через временный SQLite и подменённый Telegram transport, команды через ответы или отказы, диагностику через JSON и exit codes. Реальные Linux filesystems, права и systemd проверять отдельно от локальных macOS checks. При обновлении сохранять конфигурацию и данные установки, удерживать версию для отката.
