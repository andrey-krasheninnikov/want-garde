# ADR-0003: Shared wire contracts / Общие wire-контракты

Status / Статус: Accepted / Принято.

## Context / Контекст

Four control interfaces must agree about events, incidents, grants, typed requests, jobs, and failures without duplicating policy.

Четыре управляющих интерфейса должны одинаково понимать события, инциденты, grants, типизированные запросы, jobs и ошибки без копирования политик.

## Decision / Решение

Rust DTOs own wire version 1. Serde defines JSON encoding; schemars and ts-rs derive JSON Schema and TypeScript. An explicit export command writes artifacts; check mode compares in memory without rewriting. Committed examples define observable positive/negative cases. No auto-export tests modify tracked files.

Rust DTO владеют wire version 1. Serde определяет JSON, schemars и ts-rs выводят JSON Schema и TypeScript. Явная команда экспорта записывает артефакты; check mode сравнивает их в памяти без перезаписи. Примеры в репозитории задают наблюдаемые положительные и отрицательные сценарии. Тесты не переписывают отслеживаемые файлы.

Request decoding rejects unknown versions/actions, unexpected fields, and malformed IDs. Identity and grants are authenticated context, never client authorization claims. Core identifiers use strings to preserve precision across JavaScript. Errors use stable codes and message keys, not secret-bearing diagnostics. The initial action vocabulary is deliberately limited to a status query; later feature tickets add typed variants and results.

Декодирование отклоняет неизвестные версии/действия, лишние поля и некорректные ID. Identity и grants относятся к подтверждённому контексту, а не клиентским утверждениям о правах. ID передаются строками для сохранения точности в JavaScript. Ошибки имеют стабильные codes и message keys без секретных diagnostics. Начальный набор действий ограничен запросом статуса; новые типизированные варианты и результаты добавляются в профильных тикетах.

## Alternatives / Альтернативы

Hand-maintained Rust and TypeScript copies drift. A separate schema-first compiler adds a second source language. Opaque action maps cannot constrain administrative input.

Ручные копии Rust и TypeScript расходятся. Отдельный schema-first compiler добавляет второй исходный язык. Непрозрачные action maps не ограничивают административный ввод.

## Consequences / Последствия

Generators are locked dependencies. Any contract edit must update derived files, examples, and consumer typechecks together. Wire breaking changes require an explicit version decision; product components must reject incompatible major versions. No runtime authorization is claimed by DTO validation.

Генераторы закреплены зависимостями. Изменение контракта обновляет производные файлы, примеры и typechecks потребителей вместе. Несовместимое wire-изменение требует явного решения о версии; компоненты отклоняют несовместимые major versions. Проверка DTO не объявляется runtime-авторизацией.

Revisit when a real consumer demonstrates a representation or compatibility limitation.

Пересматривать, когда реальный потребитель покажет ограничение представления или совместимости.
