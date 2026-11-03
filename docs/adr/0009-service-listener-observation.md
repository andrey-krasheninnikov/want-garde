# ADR-0009: Services and listener baselines / Службы и baseline listeners

Status / Статус: Accepted / Принято.

## Decision / Решение

Observe an explicit list of systemd services. Distinguish inactive or failed services from unavailable observations. Persist down/recovered transitions and delivery intent together. Transitional states and missing data never clear an existing failure.

Наблюдать явный список systemd-служб. Отличать inactive/failed от недоступного наблюдения. Сохранять переходы down/recovered вместе с намерением доставки. Переходные состояния и отсутствие данных не снимают существующее падение.

The first complete endpoint snapshot becomes a persistent baseline with an initialization event. Later observations never replace it. Keep the previous observation separately to suppress repeated changes. Explicit local acceptance binds the replacement to an exact preview hash, uses the daemon writer lock, and persists its audit and notification atomically. Remote baseline changes are outside this implementation.

Первый полный снимок endpoints становится сохраняемым baseline с событием инициализации. Последующие наблюдения его не заменяют. Предыдущее наблюдение хранится отдельно для подавления повторов. Явное локальное принятие связывает замену с hash точного preview, использует writer lock daemon и атомарно сохраняет аудит с уведомлением. Удалённая замена baseline не входит в реализацию.

Use a separate socket-activated read helper for host-network listeners. Its only command is fixed ss enumeration of IPv4/IPv6 TCP/UDP listeners. Authenticate the daemon UID, bound time/output/connections, block process-control syscalls, and hide installation secrets. Keep the existing fail2ban helper network isolation. The daemon receives no additional privileges. Incomplete process metadata remains visible as a coverage limitation.

Использовать отдельный read helper с socket activation для listeners сети хоста. Единственная команда: фиксированное чтение IPv4/IPv6 TCP/UDP listeners через ss. Проверять UID daemon, ограничивать время/вывод/соединения, блокировать process-control syscalls и закрывать секреты установки. Сохранить сетевую изоляцию существующего fail2ban helper. Daemon не получает новых привилегий. Неполные метаданные процессов остаются видимым ограничением покрытия.

Process-name heuristics are warnings with an explicit reason, independent of baseline acceptance and endpoint noise filters. Store no process arguments or environment. Keep recipient access owner-only in private Telegram chats until the dedicated authorization work.

Эвристики имён процессов дают предупреждение с явной причиной независимо от принятия baseline и фильтров шума endpoints. Не сохранять аргументы или окружение процессов. Доступ получателя остаётся ограничен владельцем в личном Telegram-чате до отдельной реализации авторизации.

## Acceptance / Приёмка

Test observations through real temporary SQLite and a replacement Telegram transport, replies through authorization, and local commands through JSON/exit codes. Verify systemd, listener permissions and helper isolation separately on Linux. Rollout preserves configuration, language, data and a previous version; only the agreed service selection is added.

Проверять наблюдения через настоящий временный SQLite и подменённый Telegram transport, ответы через авторизацию, локальные команды через JSON/exit codes. Отдельно проверить systemd, права listeners и изоляцию helper на Linux. При обновлении сохраняются настройки, язык, данные и предыдущая версия; добавляется только согласованный список служб.
