# ADR-0001: Modular monorepo / Модульный монорепозиторий

Status / Статус: Accepted / Принято.

## Context / Контекст

One self-hosted installation has a small resource budget and one compatible release. Rust owns server behavior; React and Astro serve different interfaces.

Установка self-hosted имеет небольшой бюджет ресурсов и единый совместимый релиз. Rust владеет поведением сервера; React и Astro решают разные интерфейсные задачи.

## Decision / Решение

Use a modular daemon with shared contracts, four Rust executable entrypoints, a React TMA, and static Astro landing. Cargo workspaces and npm workspaces own dependency resolution and their lockfiles. Extract another Rust library only for implemented shared behavior.

Использовать модульный daemon, общие контракты, четыре Rust entrypoints, React TMA и статический Astro-лендинг. Cargo workspaces и npm workspaces управляют зависимостями и lockfiles. Новую Rust-библиотеку выделять только для реализованного общего поведения.

## Alternatives / Альтернативы

Microservices require additional deployment, IPC, and recovery paths without independent scaling needs. A single language would discard the agreed Rust/React/Astro stack. Nx, Turbo, and Bazel add orchestration that native workspaces currently cover.

Микросервисы добавляют deployment, IPC и восстановление без потребности в независимом масштабировании. Один язык отменяет согласованный стек Rust/React/Astro. Nx, Turbo и Bazel добавляют оркестрацию, которую сейчас покрывают нативные workspaces.

## Consequences / Последствия

One repository and release simplify synchronized changes. Cargo and npm still need separate commands; CI orchestrates them explicitly. No remote cache is configured before build measurements justify it.

Один репозиторий и релиз упрощают синхронные изменения. Cargo и npm требуют отдельных команд; CI вызывает их явно. Remote cache не настраивается до измерений, подтверждающих необходимость.

Revisit when measured build time or independent runtime scaling creates a concrete need.

Пересмотреть при измеренной проблеме времени сборки или необходимости независимого runtime scaling.
