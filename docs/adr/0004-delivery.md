# ADR-0004: Git, checks and releases / Git, проверки и релизы

Status / Статус: Accepted / Принято.

## Context / Контекст

One product version must identify compatible server, clients, integration packages, and TMA. Published installation artifacts must remain attributable to an immutable source revision.

Одна версия продукта должна обозначать совместимые сервер, клиенты, integration packages и TMA. Опубликованные артефакты установки должны однозначно соответствовать неизменяемой ревизии исходников.

## Decision / Решение

Use short-lived task branches from refreshed origin/main and PRs into main. Keep Conventional Commits atomic. Read back published state and exact reviewed SHAs. Remove accepted disposable remote branches; persistent task worktrees remain until separately requested cleanup. Never move published tags or rewrite accepted history.

Использовать короткие task branches от обновлённого origin/main и PR в main. Conventional Commits остаются атомарными. После публикации проверять состояние и точные reviewed SHA. Удалять принятые временные remote branches; постоянные task worktrees сохранять до отдельного запроса очистки. Не перемещать опубликованные теги и не переписывать принятую историю.

The Cargo workspace version is canonical. All npm packages mirror it and CI rejects mismatches. Initial version 0.1.0-dev.0 is unreleased. A future vMAJOR.MINOR.PATCH tag must equal the manifest version and point to the reviewed commit.

Каноническая версия находится в Cargo workspace. Все npm-пакеты повторяют её; CI отклоняет расхождения. Начальная 0.1.0-dev.0 не является релизом. Будущий vMAJOR.MINOR.PATCH tag должен совпадать с manifest version и указывать на reviewed commit.

Current CI runs formatting, lint/type checks, contract consistency, tests, and native Ubuntu builds with locked dependencies and read-only permissions. Future release automation builds documented verified Linux targets and packages the compatible binaries, TMA, skills, and Warp payloads. A release manifest records version, commit, targets, asset sizes, and SHA-256; GitHub artifact attestations bind assets to repository/tag/workflow. Installers must verify authenticity and digest before execution. Never overwrite an existing release asset. These publication jobs belong to #32, not the scaffold.

Текущий CI выполняет форматирование, lint/type checks, согласованность контрактов, tests и native Ubuntu builds с lockfiles и read-only permissions. Будущая release automation собирает документированные проверенные Linux targets и пакует совместимые binaries, TMA, skills и Warp. Release manifest содержит version, commit, targets, размеры и SHA-256; GitHub artifact attestations связывают артефакты с repository/tag/workflow. Установщики проверяют подлинность и digest до исполнения. Опубликованный release asset не перезаписывается. Эти publication jobs относятся к #32, а не каркасу.

## Alternatives / Альтернативы

Independent component versions multiply compatibility combinations. Mutable latest assets cannot identify an installation reliably. Checksums alone detect corruption but do not establish the publisher's identity.

Независимые версии компонентов увеличивают число сочетаний совместимости. Изменяемые latest assets не позволяют надёжно идентифицировать установку. Checksums показывают повреждение, но не подтверждают издателя.

## Consequences / Последствия

Coordinated releases require version checks and full applicable validation. Release failure uses a new patch/tag rather than replacement. Actual VM acceptance remains separate from builds.

Совместный релиз требует проверки версий и всех применимых проверок. Ошибка релиза исправляется новым patch/tag вместо замены. Реальная VM-приёмка остаётся отдельной от сборок.

Revisit version coupling only after independently deployed consumers establish a need.

Пересматривать общий versioning только при появлении обоснованно независимых потребителей.
