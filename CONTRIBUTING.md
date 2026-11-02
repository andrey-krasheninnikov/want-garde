# Contributing / Участие в разработке

## Русский

1. Прочитайте [правила](AGENTS.md), [словарь](GLOSSARY.md), [архитектуру](docs/architecture.md) и относящиеся к задаче [ADR](docs/adr/0001-monorepo.md).
2. Начинайте с GitHub Issue без незавершённых блокировок. Зафиксируйте scope и критерии приёмки.
3. Обновите origin/main. Создайте короткую ветку вида chore/2-monorepo-architecture или feat/3-host-status в отдельном постоянном worktree.
4. Пишите небольшие Conventional Commits: docs, feat, fix, test, build, ci, chore. Не смешивайте unrelated formatting и поведение.
5. Публикуйте Draft PR в main с номером Issue, результатом и доказательствами проверок. Readback подтверждает source/target, SHA и текст.
6. Проведите review Standards/Spec на точном кандидате. Исправления требуют повторной проверки только затронутого поведения.
7. После приёмки и разрешённого merge закройте Issue и удалите принятую временную remote branch. Постоянный task worktree сохраняется до отдельного запроса очистки.

Все authored документы, PR и Issue comments содержат RU/EN. Не публикуйте частные журналы, ID, runtime базы, токены и секреты. Не переписывайте существующую историю и опубликованные теги.

## English

1. Read the [rules](AGENTS.md), [glossary](GLOSSARY.md), [architecture](docs/architecture.md), and relevant [ADRs](docs/adr/0001-monorepo.md).
2. Start from a GitHub issue with no incomplete blockers. Record scope and acceptance criteria.
3. Refresh origin/main. Create a short-lived branch such as chore/2-monorepo-architecture or feat/3-host-status in a separate persistent worktree.
4. Use small Conventional Commits: docs, feat, fix, test, build, ci, chore. Keep unrelated formatting separate from behavior.
5. Publish a Draft PR into main with the issue number, outcome, and validation evidence. Read back source/target, SHA, and prose.
6. Review Standards/Spec against the exact candidate. Fixes invalidate only affected checks.
7. After acceptance and authorized merge, close the issue and delete the accepted disposable remote branch. Retain the persistent task worktree until a separate cleanup request.

Authored documents, PRs, and issue comments contain RU/EN. Never publish private logs, IDs, runtime databases, tokens, or secrets. Never rewrite existing history or published tags.

## Development contract / Контракт разработки

Toolchains: Rust 1.93.0; Node.js 24; npm 11. Cargo.lock and package-lock.json are committed. Install with npm ci --ignore-scripts and use cargo commands with --locked after the initial lockfile exists.

Toolchains: Rust 1.93.0; Node.js 24; npm 11. Cargo.lock и package-lock.json сохраняются в Git. Зависимости устанавливаются через npm ci --ignore-scripts; после первого lockfile Cargo запускается с --locked.

The following commands are the common local/CI checks. / Общие локальные/CI проверки:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check --workspace --all-targets --locked
cargo xtask contracts --check
cargo xtask versions
cargo test --workspace --locked
cargo build --workspace --release --locked
npm run format:check
npm run check
npm test
npm run build
git diff --check
```

Export contract changes explicitly with cargo xtask contracts. Tests and --check never update tracked artifacts. Use focused integration-test files while developing; run the full applicable suite once before publication. Contract decoding and tooling are the #2 TDD seams. SQLite/policy-to-delivery behavior and Linux VM effects arrive with their implementation tickets.

Изменения контрактов экспортируются явно через cargo xtask contracts. Tests и --check не обновляют отслеживаемые артефакты. При разработке запускайте отдельные integration-test files; перед публикацией выполните полный применимый набор один раз. Seams TDD в #2: декодирование контрактов и tooling. Поведение SQLite/policy-to-delivery и Linux VM effects проверяются в соответствующих implementation tickets.

For local previews, run npm run dev:tma or npm run dev:landing. Servers bind to 127.0.0.1. These screens are development scaffolds without Telegram authentication or server access. Stop only the preview processes started for your task.

Для локального просмотра используйте npm run dev:tma или npm run dev:landing. Серверы слушают 127.0.0.1. Эти экраны являются каркасом без Telegram authentication и доступа к серверу. Останавливайте только процессы просмотра своей задачи.

No deployment or release follows from a passing build. A release requires the evidence and immutable-artifact checks in [ADR-0004](docs/adr/0004-delivery.md).

Успешная сборка не означает deployment или release. Релиз требует доказательств и проверок неизменяемых артефактов из [ADR-0004](docs/adr/0004-delivery.md).

## Entrypoints / Точки входа

The binaries are want-garde-daemon, want-garde, want-garde-helper, and want-garde-mcp. Each supports --version and --lang LOCALE --help. The daemon accepts --config and --check-config; the CLI provides owner bind. See [installation](docs/installation.md) for configuration and exit codes. Helper/MCP reject operational invocations with exit code 2 and no stdout. The shared catalog covers 12 languages; unknown presentation locale input falls back to English.

Binaries: want-garde-daemon, want-garde, want-garde-helper и want-garde-mcp. Каждый поддерживает --version и --lang LOCALE --help. Daemon принимает --config и --check-config; CLI предоставляет owner bind. Конфигурация и exit codes описаны в [установке](docs/installation.md). Helper/MCP отклоняют операционные вызовы кодом 2 без stdout. Каталог покрывает 12 языков; неизвестная локаль представления получает английский fallback.

Development previews: http://127.0.0.1:5173/?lang=ru and http://127.0.0.1:4321/ru/. Landing routes cover all locale identifiers. Unknown static paths return 404. See [wire contracts](docs/contracts.md) before changing shared DTOs.

Локальные страницы: http://127.0.0.1:5173/?lang=ru и http://127.0.0.1:4321/ru/. Лендинг имеет маршруты всех локалей. Неизвестные статические пути возвращают 404. Перед изменением DTO прочитайте [wire-контракты](docs/contracts.md).

Astro 7 may keep its preview in the background. Stop the project preview with npm exec --workspace @want-garde/landing -- astro dev stop; do not kill other Node processes. Vite stops with Ctrl-C.

Astro 7 может оставить preview в фоне. Остановите preview проекта через npm exec --workspace @want-garde/landing -- astro dev stop; не завершайте чужие Node-процессы. Vite останавливается через Ctrl-C.

The #3 TDD seams are Telegram update to reply/denial and process startup to diagnostics/exit. Tests replace external host/Telegram sources and use temporary private files. Real Linux systemd/permissions acceptance is separate from native macOS tests.

Seams TDD в #3: Telegram update с ответом/отказом и запуск процесса с диагностикой/завершением. Tests подменяют внешние источники хоста/Telegram и используют временные закрытые файлы. Linux-проверка systemd/прав учитывается отдельно от native macOS tests.
