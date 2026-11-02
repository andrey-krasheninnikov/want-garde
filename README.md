# Want Garde

[Русский](#ru) · [English](#en)

<a id="ru"></a>

## Русский

Want Garde: открытый self-hosted продукт для мониторинга состояния и безопасности VPS и управления сервером через Telegram. Каждая установка использует собственного бота и не зависит от центрального управляющего сервиса.

**Статус: архитектура; подготовка рабочего каркаса.** Репозиторий содержит правила работы, архитектуру, спецификацию и roadmap. Рабочий Rust-бот, bootstrap-установщик, TMA и лендинг ещё не выпущены.

[Спецификация](https://github.com/andrey-krasheninnikov/want-garde/issues/1) · [Тикеты](https://github.com/andrey-krasheninnikov/want-garde/issues)

### Что запланировано

- Перенос всех возможностей Python-прототипа на Rust: SSH, firewall/fail2ban, x-ui, службы, порты, ресурсы, история и надёжная доставка уведомлений.
- Расширенный аудит, объяснимая классификация и связанные инциденты.
- Управление службами, Docker, обновлениями ОС, пользователями и SSH-ключами с разрешениями, подтверждением и TOTP.
- Shell-команды от отдельного непривилегированного пользователя с ограниченным выводом и отменой, без живого PTY.
- Подключение к Telegram через готовые VLESS/WireGuard/Tailscale маршруты, CLI/MCP по SSH, эксплуатационные skills и интеграция Warp.
- Bootstrap CLI с настройками, проверкой и установкой обновлений, откатом продукта и зашифрованными резервными копиями; React TMA; Astro-лендинг с синтетическим демо и Telegram-контактом для кастомных решений.

### Платформы и языки

Целевая матрица: Debian, Ubuntu LTS, Rocky/AlmaLinux, Fedora, Arch и openSUSE, x86_64/ARM64. Поддерживаемые сочетания будут подтверждены на Linux VM. Цель базового профиля: VPS с 1 vCPU и 512 MiB RAM; это требование для будущих измерений.

Интерфейсы и лендинг: `ru`, `en`, `zh-Hans`, `es`, `pt-BR`, `hi`, `ja`, `ar`, `he`, `tr`, `fr`, `de`, включая RTL для арабского и иврита. Документация, спецификация и тикеты ведутся на русском и английском.

[Разработка](CONTRIBUTING.md) · [Архитектура](docs/architecture.md) · [Словарь](GLOSSARY.md)

### Работа с проектом

Архитектура монорепозитория идёт первой. Bootstrap, TMA и лендинг находятся в конце roadmap. Работу начинают с тикета, чьи блокировки сняты; критерии приёмки находятся в GitHub Issues.

Правила: [AGENTS.md](AGENTS.md). Настройки engineering skills: [трекер](docs/agents/issue-tracker.md), [triage labels](docs/agents/triage-labels.md), [domain docs](docs/agents/domain.md). Настройки можно менять непосредственно в этих документах.

<a id="en"></a>

## English

Want Garde is an open-source self-hosted product for VPS health and security monitoring and server management through Telegram. Each installation uses its own bot and does not depend on a central control service.

**Status: architecture; scaffold in progress.** This repository contains workflow rules, architecture, the specification, and roadmap. The working Rust bot, bootstrap installer, TMA, and landing page have not been released yet.

[Specification](https://github.com/andrey-krasheninnikov/want-garde/issues/1) · [Tickets](https://github.com/andrey-krasheninnikov/want-garde/issues)

### Planned capabilities

- Port all Python-prototype capabilities to Rust: SSH, firewall/fail2ban, x-ui, services, ports, resources, history, and reliable notification delivery.
- Extended auditing, explainable classification, and correlated incidents.
- Manage services, Docker, OS updates, users, and SSH keys with permissions, confirmation, and TOTP.
- Run shell commands as a separate unprivileged user with bounded output and cancellation, without a live PTY.
- Reach Telegram through existing VLESS/WireGuard/Tailscale routes; use CLI/MCP over SSH, operational skills, and the Warp integration.
- Bootstrap CLI with configuration, update checks and installation, product rollback, and encrypted backups; React TMA; Astro landing page with a synthetic demo and Telegram contact for custom development.

### Platforms and languages

Target matrix: Debian, Ubuntu LTS, Rocky/AlmaLinux, Fedora, Arch, and openSUSE on x86_64/ARM64. Supported combinations will be verified on Linux VMs. The basic-profile target is a VPS with 1 vCPU and 512 MiB RAM; this is a requirement for future measurements.

Interfaces and landing page: `ru`, `en`, `zh-Hans`, `es`, `pt-BR`, `hi`, `ja`, `ar`, `he`, `tr`, `fr`, and `de`, including Arabic and Hebrew RTL. Documentation, the specification, and tickets are maintained in Russian and English.

[Development](CONTRIBUTING.md) · [Architecture](docs/architecture.md) · [Glossary](GLOSSARY.md)

### Working on the project

Monorepo architecture comes first. Bootstrap, TMA, and the landing page are the final roadmap groups. Start with a ticket whose blockers are complete; acceptance criteria live in GitHub Issues.

Rules: [AGENTS.md](AGENTS.md). Engineering-skill configuration: [issue tracker](docs/agents/issue-tracker.md), [triage labels](docs/agents/triage-labels.md), and [domain docs](docs/agents/domain.md). These settings can be edited directly.

## Roadmap / План реализации

| № | Тикет / Ticket | Блокируют / Blocked by |
| --- | --- | --- |
| 1 | [Архитектура монорепозитория и Git-процесс / Design the monorepo architecture and Git workflow](https://github.com/andrey-krasheninnikov/want-garde/issues/2) | Нет / None |
| 2 | [Первый Rust-бот: запуск и /status / Run the first Rust bot and expose server status](https://github.com/andrey-krasheninnikov/want-garde/issues/3) | [#2](https://github.com/andrey-krasheninnikov/want-garde/issues/2) |
| 3 | [Надёжная доставка событий через SQLite / Persist events and deliver them reliably through SQLite](https://github.com/andrey-krasheninnikov/want-garde/issues/4) | [#3](https://github.com/andrey-krasheninnikov/want-garde/issues/3) |
| 4 | [Мониторинг SSH и восстановление чтения журналов / Monitor SSH and resume journal ingestion after interruptions](https://github.com/andrey-krasheninnikov/want-garde/issues/5) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 5 | [События firewall и fail2ban / Collect firewall and fail2ban events](https://github.com/andrey-krasheninnikov/want-garde/issues/6) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 6 | [CPU, RAM, диски и состояние хоста / Monitor CPU, memory, disks, inodes, and host health](https://github.com/andrey-krasheninnikov/want-garde/issues/7) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 7 | [Мониторинг служб и listening ports / Monitor services, listening ports, and persistent baselines](https://github.com/andrey-krasheninnikov/want-garde/issues/8) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 8 | [Паритет мониторинга x-ui/3x-ui / Preserve x-ui and 3x-ui monitoring behavior](https://github.com/andrey-krasheninnikov/want-garde/issues/9) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 9 | [Необязательное GeoIP-обогащение / Add optional GeoIP context with bounded external requests](https://github.com/andrey-krasheninnikov/want-garde/issues/10) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 10 | [История, диагностика и ограничения хранения / Expose history, diagnostics, retention, and storage limits](https://github.com/andrey-krasheninnikov/want-garde/issues/11) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 11 | [Личные уведомления и forum topics / Route private notifications and forum topics with privacy controls](https://github.com/andrey-krasheninnikov/want-garde/issues/12) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 12 | [Подавление шума, тихие часы и сводки / Reduce alert noise and provide quiet hours and summaries](https://github.com/andrey-krasheninnikov/want-garde/issues/13) | [#12](https://github.com/andrey-krasheninnikov/want-garde/issues/12) |
| 13 | [Telegram через существующие VPN и прокси / Connect Telegram through existing VPN routes and proxies](https://github.com/andrey-krasheninnikov/want-garde/issues/14) | [#4](https://github.com/andrey-krasheninnikov/want-garde/issues/4) |
| 14 | [Права на функции и команды в группах / Enforce per-function permissions and group command access](https://github.com/andrey-krasheninnikov/want-garde/issues/15) | [#12](https://github.com/andrey-krasheninnikov/want-garde/issues/12) |
| 15 | [TOTP, подтверждения и безопасный fail2ban unban / Confirm privileged actions with TOTP and safely unban fail2ban IPs](https://github.com/andrey-krasheninnikov/want-garde/issues/16) | [#6](https://github.com/andrey-krasheninnikov/want-garde/issues/6), [#15](https://github.com/andrey-krasheninnikov/want-garde/issues/15) |
| 16 | [Управление systemd-службами / Control permitted systemd services](https://github.com/andrey-krasheninnikov/want-garde/issues/17) | [#16](https://github.com/andrey-krasheninnikov/want-garde/issues/16) |
| 17 | [Просмотр Docker и диагностика / Inspect Docker images, containers, resources, and logs](https://github.com/andrey-krasheninnikov/want-garde/issues/18) | [#16](https://github.com/andrey-krasheninnikov/want-garde/issues/16) |
| 18 | [Start/stop/restart Docker-контейнеров / Start, stop, and restart permitted Docker containers](https://github.com/andrey-krasheninnikov/want-garde/issues/19) | [#18](https://github.com/andrey-krasheninnikov/want-garde/issues/18) |
| 19 | [Создание пользователей и SSH-ключи / Create regular users and manage public SSH keys](https://github.com/andrey-krasheninnikov/want-garde/issues/20) | [#16](https://github.com/andrey-krasheninnikov/want-garde/issues/16) |
| 20 | [Проверка и установка обновлений ОС / Check and install operating-system updates](https://github.com/andrey-krasheninnikov/want-garde/issues/21) | [#16](https://github.com/andrey-krasheninnikov/want-garde/issues/16) |
| 21 | [Контролируемая перезагрузка VPS / Reboot a VPS with explicit confirmation and recovery reporting](https://github.com/andrey-krasheninnikov/want-garde/issues/22) | [#21](https://github.com/andrey-krasheninnikov/want-garde/issues/21) |
| 22 | [Shell-команды, пагинация и less / Run shell commands with bounded output, pagination, and less handling](https://github.com/andrey-krasheninnikov/want-garde/issues/23) | [#16](https://github.com/andrey-krasheninnikov/want-garde/issues/16) |
| 23 | [Профили мониторинга и аудит изменений / Provide monitoring profiles and configuration-change auditing](https://github.com/andrey-krasheninnikov/want-garde/issues/24) | [#5](https://github.com/andrey-krasheninnikov/want-garde/issues/5), [#7](https://github.com/andrey-krasheninnikov/want-garde/issues/7), [#8](https://github.com/andrey-krasheninnikov/want-garde/issues/8), [#9](https://github.com/andrey-krasheninnikov/want-garde/issues/9), [#16](https://github.com/andrey-krasheninnikov/want-garde/issues/16) |
| 24 | [Полный профиль: аудит процессов и привилегий / Audit process execution and privilege events in the full profile](https://github.com/andrey-krasheninnikov/want-garde/issues/25) | [#24](https://github.com/andrey-krasheninnikov/want-garde/issues/24) |
| 25 | [Инциденты: корреляция, подтверждение и закрытие / Correlate events into incidents with acknowledgement and closure](https://github.com/andrey-krasheninnikov/want-garde/issues/26) | [#5](https://github.com/andrey-krasheninnikov/want-garde/issues/5), [#11](https://github.com/andrey-krasheninnikov/want-garde/issues/11), [#15](https://github.com/andrey-krasheninnikov/want-garde/issues/15) |
| 26 | [Расширенные правила классификации / Expand explainable event and incident classification rules](https://github.com/andrey-krasheninnikov/want-garde/issues/27) | [#25](https://github.com/andrey-krasheninnikov/want-garde/issues/25), [#26](https://github.com/andrey-krasheninnikov/want-garde/issues/26) |
| 27 | [CLI управления с JSON-контрактом / Expose a shared JSON control CLI](https://github.com/andrey-krasheninnikov/want-garde/issues/28) | [#10](https://github.com/andrey-krasheninnikov/want-garde/issues/10), [#13](https://github.com/andrey-krasheninnikov/want-garde/issues/13), [#14](https://github.com/andrey-krasheninnikov/want-garde/issues/14), [#17](https://github.com/andrey-krasheninnikov/want-garde/issues/17), [#19](https://github.com/andrey-krasheninnikov/want-garde/issues/19), [#20](https://github.com/andrey-krasheninnikov/want-garde/issues/20), [#22](https://github.com/andrey-krasheninnikov/want-garde/issues/22), [#23](https://github.com/andrey-krasheninnikov/want-garde/issues/23), [#27](https://github.com/andrey-krasheninnikov/want-garde/issues/27) |
| 28 | [MCP через stdio/SSH / Serve MCP tools over stdio and SSH](https://github.com/andrey-krasheninnikov/want-garde/issues/29) | [#28](https://github.com/andrey-krasheninnikov/want-garde/issues/28) |
| 29 | [Универсальные skills и установщик / Publish portable operational skills and their installer](https://github.com/andrey-krasheninnikov/want-garde/issues/30) | [#29](https://github.com/andrey-krasheninnikov/want-garde/issues/29) |
| 30 | [Интеграция Warp / Package the Want Garde integration for Warp](https://github.com/andrey-krasheninnikov/want-garde/issues/31) | [#30](https://github.com/andrey-krasheninnikov/want-garde/issues/30) |
| 31 | [Приёмка ядра на Linux VM и релизные артефакты / Validate the core on Linux VMs and produce verifiable release artifacts](https://github.com/andrey-krasheninnikov/want-garde/issues/32) | [#31](https://github.com/andrey-krasheninnikov/want-garde/issues/31) |
| 32 | [Bootstrap: одна команда и мастер настройки / Bootstrap Want Garde with one command and a setup wizard](https://github.com/andrey-krasheninnikov/want-garde/issues/33) | [#32](https://github.com/andrey-krasheninnikov/want-garde/issues/32) |
| 33 | [CLI настройки и диагностики / Manage configuration and diagnostics through the CLI](https://github.com/andrey-krasheninnikov/want-garde/issues/34) | [#33](https://github.com/andrey-krasheninnikov/want-garde/issues/33) |
| 34 | [Резервное копирование и восстановление Want Garde / Back up, restore, and transfer encrypted Want Garde data](https://github.com/andrey-krasheninnikov/want-garde/issues/35) | [#34](https://github.com/andrey-krasheninnikov/want-garde/issues/34) |
| 35 | [CLI проверки обновлений, self-update и откат / Check updates, update Want Garde, and roll back through the CLI](https://github.com/andrey-krasheninnikov/want-garde/issues/36) | [#35](https://github.com/andrey-krasheninnikov/want-garde/issues/35) |
| 36 | [Удаление установки с сохранением выбранных данных / Uninstall only installation-owned components and optionally retain data](https://github.com/andrey-krasheninnikov/want-garde/issues/37) | [#33](https://github.com/andrey-krasheninnikov/want-garde/issues/33) |
| 37 | [React TMA: HTTPS, авторизация и статус / Serve a localized React TMA with HTTPS authentication and host status](https://github.com/andrey-krasheninnikov/want-garde/issues/38) | [#33](https://github.com/andrey-krasheninnikov/want-garde/issues/33) |
| 38 | [TMA: мониторинг, история и инциденты / Inspect monitoring, history, and incidents in the TMA](https://github.com/andrey-krasheninnikov/want-garde/issues/39) | [#38](https://github.com/andrey-krasheninnikov/want-garde/issues/38) |
| 39 | [TMA: управление сервером / Manage services, Docker, updates, users, and SSH keys in the TMA](https://github.com/andrey-krasheninnikov/want-garde/issues/40) | [#38](https://github.com/andrey-krasheninnikov/want-garde/issues/38) |
| 40 | [TMA: выполнение команд и результаты / Run commands and inspect bounded results in the TMA](https://github.com/andrey-krasheninnikov/want-garde/issues/41) | [#38](https://github.com/andrey-krasheninnikov/want-garde/issues/38) |
| 41 | [TMA: настройки, права и резервные копии / Manage TMA settings, permissions, VPN, and encrypted backups](https://github.com/andrey-krasheninnikov/want-garde/issues/42) | [#36](https://github.com/andrey-krasheninnikov/want-garde/issues/36), [#38](https://github.com/andrey-krasheninnikov/want-garde/issues/38) |
| 42 | [Интерактивное демо на синтетических данных / Provide an interactive demo using synthetic data](https://github.com/andrey-krasheninnikov/want-garde/issues/43) | [#39](https://github.com/andrey-krasheninnikov/want-garde/issues/39), [#40](https://github.com/andrey-krasheninnikov/want-garde/issues/40), [#41](https://github.com/andrey-krasheninnikov/want-garde/issues/41), [#42](https://github.com/andrey-krasheninnikov/want-garde/issues/42) |
| 43 | [Astro-лендинг: 12 языков, SEO и все целевые действия / Launch an Astro landing page with demo, installation, GitHub, and custom-work contact](https://github.com/andrey-krasheninnikov/want-garde/issues/44) | [#37](https://github.com/andrey-krasheninnikov/want-garde/issues/37), [#43](https://github.com/andrey-krasheninnikov/want-garde/issues/43) |

## License / Лицензия

[MIT](LICENSE).
