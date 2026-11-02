# ADR-0002: Privilege separation / Разделение привилегий

Status / Статус: Accepted / Принято.

## Context / Контекст

Remote administration must not turn a Telegram account, an integration, or a shell command into unrestricted root access.

Удалённое управление не должно превращать Telegram-аккаунт, интеграцию или shell-команду в неограниченный root-доступ.

## Decision / Решение

The daemon owns authorization and state. CLI/MCP use a protected Unix socket; TMA uses a separately authenticated optional HTTPS adapter. The helper authenticates its peer and independently constrains typed actions and target resources with root-owned policy. Arbitrary shell runs as a separate user, never as the helper's root identity.

Daemon владеет авторизацией и состоянием. CLI/MCP используют защищённый Unix socket; TMA использует отдельно аутентифицированный необязательный HTTPS adapter. Helper проверяет peer и независимо ограничивает типизированные действия и ресурсы root-owned policy. Произвольный shell работает от отдельного пользователя, а не root identity helper.

Approval, TOTP, durable audit, replay protection, and reconciliation are mandatory for dangerous operations. Integration approval never broadens original scope. Helper policy protects the privilege boundary; complete daemon compromise remains outside the daemon's own trust guarantee.

Подтверждение, TOTP, сохраняемый аудит, защита от повторов и reconciliation обязательны для опасных операций. Подтверждение интеграции не расширяет исходный scope. Политика helper защищает границу привилегий; полная компрометация daemon находится вне его собственной гарантии доверия.

## Alternatives / Альтернативы

A root daemon, a Docker-group daemon, generic sudo commands, and a privileged interactive shell expose broader authority than typed administration requires. A public MCP listener adds an unnecessary network boundary.

Root daemon, членство daemon в Docker group, произвольные sudo-команды и привилегированный интерактивный shell дают больше полномочий, чем требуется типизированному управлению. Публичный MCP listener добавляет ненужную сетевую границу.

## Consequences / Последствия

Implementation needs OS-peer checks, root-owned allowlists, bounded arguments/output, and VM tests. #16 owns concrete helper/approval implementation; #23 owns shell isolation. Compilation alone proves none of these runtime properties.

Реализация требует проверки OS peer, root-owned allowlists, ограничений параметров/вывода и VM-тестов. #16 реализует helper/approval, #23 реализует изоляцию shell. Сборка не доказывает эти runtime-свойства.

Revisit only with a documented threat-model change and new isolation evidence.

Пересматривать только при документированном изменении threat model и новых проверках изоляции.
