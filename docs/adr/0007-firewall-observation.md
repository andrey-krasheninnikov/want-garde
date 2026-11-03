# ADR-0007: Firewall observation / Наблюдение firewall

Status / Статус: Accepted / Принято.

## Decision / Решение

Fail2ban and firewall collectors use bounded journal/file readers and atomically persist events, delivery intent and source progress. First activation starts at the tail; lost progress produces a persistent coverage gap. SSH checkpoints remain compatible.

Collectors fail2ban и firewall используют ограниченное чтение журналов и атомарно сохраняют событие, доставку и позицию источника. Первое включение начинает с конца; потеря позиции создаёт сохраняемый пропуск покрытия. Позиции SSH сохраняют совместимость.

A separate root helper exposes only typed fail2ban reads over a protected Unix socket. The daemon keeps its unprivileged identity. Peer credentials, fixed executables, bounded output and deadlines constrain the helper. Administrative unban, approvals and TOTP belong to #16.

Отдельный root helper предоставляет только типизированное чтение fail2ban через защищённый Unix socket. Daemon сохраняет непривилегированную identity. Peer credentials, фиксированные executable, ограниченный вывод и сроки ограничивают helper. Административный unban, подтверждения и TOTP относятся к #16.

UFW blocks, standard firewalld drop/reject records and explicitly configured nftables log prefixes are observation sources. Readable logs alone do not establish enabled firewall logging. Only verified platform/backend combinations are described as runtime supported; the full Linux matrix belongs to #32.

Источниками наблюдений являются UFW blocks, стандартные firewalld drop/reject и явно заданные префиксы nftables. Читаемый журнал сам по себе не подтверждает включённое логирование firewall. Runtime-поддержка заявляется только для проверенных сочетаний платформы и backend; полная Linux-матрица относится к #32.

## Permissions / Права

File logging may require a service-specific supplementary group. On the current Ubuntu installation, an adm drop-in grants access to adm-readable logs without changing global account membership, fail2ban configuration or firewall rules. Installation and rollback record this broader read access explicitly.

Чтение файлов может требовать дополнительную группу только для сервиса. На текущей Ubuntu drop-in adm даёт чтение журналов этой группы без изменения глобального членства пользователя, конфигурации fail2ban и правил firewall. Установка и rollback явно учитывают этот расширенный доступ на чтение.
