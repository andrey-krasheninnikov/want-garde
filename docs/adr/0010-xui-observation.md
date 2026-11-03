# ADR-0010: x-ui observation / Наблюдение x-ui

Status / Статус: Accepted / Принято.

## Decision / Решение

Read local x-ui/3x-ui databases and logs through a separate socket-activated read helper. Root-owned policy fixes the allowed source paths and the daemon UID. Requests cannot supply SQL, commands, or arbitrary paths. The daemon remains unprivileged; panel permissions remain unchanged.

Читать локальные базы и журналы x-ui/3x-ui через отдельный read helper с socket activation. Root-owned политика задаёт разрешённые пути источников и UID daemon. Запросы не передают SQL, команды или произвольные пути. Daemon остаётся непривилегированным; права панели сохраняются.

Normalize observations before sending them to the daemon. Persist fingerprints of configuration fields, never plaintext credentials or raw log lines. Keep source availability separate from client connectivity. An unavailable or incomplete source never implies that every client disconnected.

Нормализовать наблюдения до передачи daemon. Сохранять fingerprints полей конфигурации, без открытых credentials и сырых строк журналов. Отличать доступность источника от состояния соединений клиентов. Недоступный или неполный источник не означает отключение всех клиентов.

Commit events, delivery intent, and source progress atomically through the existing SQLite writer. The first complete database snapshot establishes initial state; logs start at the tail. Later restarts resume checkpoints and expose gaps. Bound reads, response sizes, stored state, and processing time.

Сохранять события, намерение доставки и прогресс источников атомарно через существующий SQLite writer. Первый полный снимок базы задаёт исходное состояние; журналы начинают читаться с конца. Рестарты продолжают checkpoints и показывают пропуски. Ограничивать чтение, размер ответов, состояние и время обработки.

Remote panel administration is excluded. Telegram results remain owner-only in private chats. Local daemon diagnostics read saved state without a token, network requests, or database writes. Rollback restores a compatible configuration and previous binaries while retaining the current delivery database.

Удалённое управление панелью исключено. Telegram-результаты доступны владельцу в личном чате. Локальная диагностика daemon читает сохранённое состояние без токена, сетевых запросов и записи в базу. Откат восстанавливает совместимую конфигурацию и предыдущие бинарники с сохранением текущей базы доставок.

## Verification / Проверка

Use synthetic databases and logs through the real reader, temporary SQLite, and a replacement Telegram transport. Verify socket permissions and systemd isolation separately on Linux. Private panel databases and logs never become fixtures.

Использовать синтетические базы и журналы через настоящий reader, временный SQLite и подменённый Telegram transport. Отдельно проверять права socket и systemd isolation на Linux. Частные базы и журналы панели не становятся fixtures.
