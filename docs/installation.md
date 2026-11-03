# Manual installation / Ручная установка

## Русский

Сервис обслуживает один Linux VPS и отдельный Telegram bot token через HTTPS polling. В личном чате владельцу доступны `/start`, `/help`, `/status`, `/delivery`, `/ssh`, `/bans` и `/language`. Неизвестные пользователи, группы, callbacks и edited messages не получают состояние. Первичная привязка выполняется локально; username и статус администратора группы не дают доступа.

### Подготовка

Нужны systemd, исходящая HTTPS-связь с api.telegram.org и отдельный bot token. Webhook должен отсутствовать; сервис не удаляет его автоматически. Не запускайте другой polling-процесс с тем же токеном. Конфигурация проверяется до сети. Proxy-конфигурация относится к #14; текущий transport не использует proxy-переменные окружения и не меняет маршрут ОС.

Ручной workflow CI создаёт development artifact `linux-x86_64-<commit>` из точного SHA исходников. Автозапуск отключён; обязательные проверки выполняются локально. Это сборка Ubuntu 24.04 x86_64, без release tag и обещания совместимости с другими дистрибутивами. Получайте artifact только из workflow этого репозитория; сравните `revision` с проверенным SHA и выполните проверки контрольных сумм. Checksum подтверждает целостность, но сам по себе не подтверждает издателя.

```sh
sha256sum -c want-garde-linux-x86_64.tar.gz.sha256
mkdir installation
# Сначала проверьте список: только binaries, unit, config.json, revision и SHA256SUMS.
tar -tzf want-garde-linux-x86_64.tar.gz
tar -xzf want-garde-linux-x86_64.tar.gz -C installation
cd installation
sha256sum -c SHA256SUMS
cat revision
```

До создания пользователя проверьте отсутствие чужой учётной записи `want-garde`, unit и каталогов установки. Не переиспользуйте их без проверки владельца и назначения. Команды ниже относятся к новой установке. Вместо `<commit>` подставьте полный проверенный SHA; вместо `123456789` используйте свой числовой Telegram user ID.

```sh
sudo useradd --system --user-group --home-dir /var/lib/want-garde --shell /usr/sbin/nologin want-garde
sudo install -d -o root -g want-garde -m 0750 /etc/want-garde
sudo install -d -o want-garde -g want-garde -m 0700 /var/lib/want-garde
sudo install -d -o root -g root -m 0755 /usr/local/lib/want-garde/<commit>
sudo install -m 0755 want-garde want-garde-daemon /usr/local/lib/want-garde/<commit>/
sudo install -o root -g want-garde -m 0640 config.json /etc/want-garde/config.json
sudoedit /etc/want-garde/config.json
sudoedit /etc/want-garde/telegram-token
sudo chown root:want-garde /etc/want-garde/telegram-token
sudo chmod 0640 /etc/want-garde/telegram-token
sudo /usr/local/lib/want-garde/<commit>/want-garde owner bind --config /etc/want-garde/config.json --user-id 123456789
sudo -u want-garde /usr/local/lib/want-garde/<commit>/want-garde-daemon --check-config --config /etc/want-garde/config.json
sudo ln -s <commit> /usr/local/lib/want-garde/current
sudo install -m 0644 want-garde.service /etc/systemd/system/want-garde.service
sudo systemctl daemon-reload
sudo systemctl enable --now want-garde.service
```

Токен вводится редактором в отдельный файл, одной строкой. Не передавайте его через аргументы, shell history, Git или сообщения. В config задайте стабильный `server_id`, абсолютные `bot_token_file` и `state_dir`, поддерживаемый `default_locale`. Начальный `owner_user_id` равен null; CLI сохраняет десятичную строку. Повтор того же ID успешен; другой ID отклоняется. CLI сохраняет mode/group конфигурации. Не изменяйте привязку при работающем сервисе: изменения конфигурации применяются после restart.

### Проверка и остановка

```sh
sudo systemctl status want-garde.service
sudo journalctl -u want-garde.service --since '5 minutes ago' --no-pager
sudo systemctl stop want-garde.service
sudo systemctl start want-garde.service
sudo systemctl restart want-garde.service
```

После `service.ready` отправьте боту `/status` от привязанного аккаунта. Ответ содержит server ID, UTC-время снимка, hostname, ОС/ядро, версию, uptime, CPU, load за 1/5/15 минут, RAM и корневой диск. RAM и диск показывают доступный/общий объём в MiB. CPU использует два отсчёта с интервалом 200 ms; неизвестные значения показаны отдельно с reason key. Это снимок по запросу, без пороговых событий и полного профиля #7.

`/language` перечисляет `ru`, `en`, `zh-Hans`, `es`, `pt-BR`, `hi`, `ja`, `ar`, `he`, `tr`, `fr`, `de`. `/language ar` меняет язык с подтверждением. Выбор сохраняется атомарно в закрытом `owner-preferences.json` и переживает restart. Неизвестный код показывает список; повреждённый файл предпочтений вызывает явный отказ при старте. Для ar/he динамические значения изолированы Unicode bidi markers; сообщения отправляются обычным текстом без HTML/Markdown.

Сервис работает без root, capabilities, sudo и Docker socket. Токен/config читаются, состояние доступно только service user. SIGTERM/SIGINT прерывают polling и ожидание backoff. Код 2 обозначает отказ конфигурации, identity или постоянную ошибку запуска; systemd не повторяет такой запуск автоматически. После аварийного завершения CLI проверьте отсутствие живой команды перед ручным удалением оставшегося `config.lock`.

Сетевые ошибки polling повторяются с backoff 1..30 s с учётом Telegram retry_after. Ошибки токена, webhook и конфликт polling останавливают процесс. События, ответы и polling offset сохраняются в SQLite до отправки. Неоднозначная отправка может повториться с тем же ID. Срок доставки события: 24 часа; ответа: 10 минут. См. [доставку, диагностику и ограничения](delivery.md).

Для отката первой неуспешной установки остановите и отключите только `want-garde.service`; сохраните конфигурацию и токен для исправления. При последующих ручных обновлениях сначала сохраните предыдущий SHA и файлы, остановите сервис, переключите `current` на проверенную сборку и проверьте запуск. Совместимость будущих данных и автоматический rollback требуют отдельных тикетов. Read helper устанавливается отдельно по [инструкции firewall/fail2ban](firewall.md); MCP остаётся каркасом.

## English

The service runs on one Linux VPS with a dedicated Telegram bot token and HTTPS polling. The owner can use `/start`, `/help`, `/status`, `/delivery`, `/ssh`, `/bans`, and `/language` in a private chat. Unknown users, groups, callbacks, and edited messages receive no host status. Initial binding is local; usernames and group-administrator status grant no access.

### Preparation

Use systemd, outbound HTTPS access to api.telegram.org, and a dedicated token without a webhook. The service never removes a webhook automatically. Do not run another poller with the same token. Local configuration is checked before network access. Proxy configuration belongs to #14; the current transport ignores proxy environment variables and leaves OS routing unchanged.

The manual CI workflow produces a development artifact named `linux-x86_64-<commit>` from the exact source SHA. Automatic triggers are disabled; required checks run locally. It is an Ubuntu 24.04 x86_64 build, without a release tag or a compatibility promise for other distributions. Download only from this repository's workflow. Use the checksum and extraction commands above, inspect the archive entries before extraction, and compare `revision` with the reviewed full SHA. Checksums establish integrity, not publisher identity by themselves.

The installation commands above apply to a new installation. Inspect any existing `want-garde` account, unit, or directory before reuse. Replace `<commit>` with the reviewed full SHA and `123456789` with your Telegram user ID. The commands create a separate system account, restricted configuration/token files, a private state directory, an immutable binary directory, and a `current` symlink. Enable the unit only after `--check-config` succeeds as the service user.

Enter the token as one line using an editor, never through arguments, shell history, Git, or messages. Configure a stable `server_id`, absolute `bot_token_file` and `state_dir` paths, and a supported `default_locale`. The initial `owner_user_id` is null. The local CLI stores a decimal string, accepts the same ID idempotently, refuses another ID, and preserves configuration mode/group. Bind or change configuration while the service is stopped; restart to apply changes.

### Verification and shutdown

Use the systemctl and journalctl commands above to inspect, stop, start, and restart this unit only. After `service.ready`, send `/status` from the bound account. The response includes server ID, UTC snapshot time, hostname, OS/kernel, product version, uptime, CPU, 1/5/15-minute load, RAM, and root filesystem. RAM/disk show available/total MiB. CPU uses two samples 200 ms apart. Missing measurements have an explicit reason key. Threshold events and the complete resource profile remain in #7.

`/language` lists all 12 locale codes above. `/language ar` saves a choice atomically in the restricted `owner-preferences.json`; it survives restart. An unknown code shows the list. Corrupt preferences cause an explicit startup failure. Arabic/Hebrew output isolates dynamic values with Unicode bidi markers. Telegram messages use plain text without HTML/Markdown parsing.

The daemon has no root, capabilities, sudo, or Docker-socket access. Configuration/token files are read only; state is private to the service user. SIGTERM/SIGINT interrupt polling and backoff. Exit code 2 means invalid configuration/identity or a permanent startup failure; systemd does not restart it automatically. If a terminated CLI leaves `config.lock`, confirm that no binding command is running before removing that lock manually.

Polling network failures use a 1..30-second backoff and respect Telegram retry_after. Invalid tokens, webhooks, and polling conflicts stop the process. Events, replies, and polling progress are persisted in SQLite before sending. An ambiguous send can repeat with the same ID. Events expire after 24 hours; replies after 10 minutes. See [delivery, diagnostics, and limitations](delivery.md).

To roll back a failed first installation, stop and disable only `want-garde.service`, retaining configuration and token for correction. Before later manual updates, preserve the previous SHA/files, stop the service, switch `current` to a verified build, and check startup. Future data compatibility and automatic rollback require their implementation issues. Install the read helper separately using the [firewall/fail2ban instructions](firewall.md); MCP remains a scaffold.

## SSH

Служба получает чтение системного journal через `SupplementaryGroups=systemd-journal`; это доступ ко всем системным журналам. Первый запуск принимает только новые записи. Режимы, восстановление и `/ssh` описаны в [мониторинге SSH](ssh.md).

The service reads the system journal through `SupplementaryGroups=systemd-journal`, granting access to all system journals. The first run starts with new records. See [SSH monitoring](ssh.md) for modes, recovery, and `/ssh`.

## Firewall / Firewall

Сбор, live `/bans`, права adm и read helper описаны в [firewall/fail2ban](firewall.md). / See [firewall/fail2ban](firewall.md) for collection, live `/bans`, adm access and the read helper.

## Host health / Состояние хоста

See [host health](host-health.md) for filesystem paths, thresholds, reboot detection, and `--host-status`.

Пути файловых систем, пороги, проверка reboot и `--host-status` описаны в [состоянии хоста](host-health.md).

## Services and listeners / Службы и listeners

См. [мониторинг служб, портов и baseline](services-ports.md): конфигурация, отдельный listener helper, `/services`, `/ports`, JSON-диагностика и локальное принятие baseline.

See [service, port and baseline monitoring](services-ports.md) for configuration, the separate listener helper, `/services`, `/ports`, JSON diagnostics and local baseline acceptance.
