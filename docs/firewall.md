# Firewall and fail2ban / Firewall и fail2ban

## Русский

Want Garde сохраняет fail2ban Ban, Restore Ban и Unban, UFW BLOCK, стандартные firewalld DROP/REJECT и явно настроенные nftables-префиксы. Ban имеет важность critical, unban — info, firewall block — warning. Данные ограничены jail, IP, backend, протоколом, назначением и портом, когда они присутствуют. Сырые журналы и packet tails не сохраняются.

### Просмотр и диагностика

Владельцу в личном чате доступны `/bans`, `/bans --page 2` и `/bans sshd 2`. Общая страница содержит до 10 jails, страница jail — до 20 IP. Снимок содержит время получения, текущие счётчики и явный признак неполноты. Неполный или недоступный снимок не означает отсутствие bans. Запросы групп, посторонних пользователей и callbacks не вызывают helper. Unban относится к #16.

`/bans` также показывает здоровье обоих источников, пропуски, неподдержанные/повреждённые записи и стабильные reason keys. Локальная команда запускается от want-garde, не читает token и не меняет базу:

```sh
sudo -u want-garde want-garde-daemon --firewall-status --config /etc/want-garde/config.json
```

JSON использует состояния starting, healthy, degraded и unavailable. Exit codes: 0 — оба источника healthy, 1 — неполное или недоступное покрытие, 2 — ошибка конфигурации/базы/диагностики. Исправный reader не доказывает эффективность firewall или полноту всех сетевых блокировок: logging и rate limits источника ограничивают наблюдения.

### Источники и восстановление

Существующая конфигурация работает без новых полей. `fail2ban` принимает те же режимы auto/journald/file, что и `ssh`; `firewall` содержит `source`, `backend` и необязательный `log_prefix`.

Auto fail2ban проверяет live logtarget и loglevel через read helper. Файловый target читается из указанного файла; SYSLOG, SYSTEMD-JOURNAL, STDOUT, STDERR и SYSOUT используют journal. Уровень должен включать NOTICE: WARNING/ERROR/CRITICAL не обеспечивают сбор bans. Пока target не проверен, journal не закрепляется как источник. Поддерживаются стандартные action- и syslog-форматы fail2ban. Journal-фильтр принимает root-origin записи стандартной установки; SYSLOG_IDENTIFIER сам по себе не подтверждает источник.

Auto firewall выбирает активный UFW с включённым логированием, затем firewalld с включённым LogDenied. Для UFW используется читаемый ufw.log либо kernel journal; firewalld использует kernel journal. nftables требует явного backend и префикса, применяемого только к правилам DROP/REJECT. Сам nftables log не завершает обработку пакета. NFLOG и audit-формат в этом срезе не поддерживаются. Источники: [nftables manual](https://netfilter.org/projects/nftables/manpage.html), [firewalld LogDenied](https://firewalld.org/documentation/man-pages/firewalld.conf.html).

Пример дополнения конфигурации для обычного nftables, без автоматического изменения правил:

```json
{
  "firewall": {
    "source": { "mode": "journald" },
    "backend": "nftables",
    "log_prefix": "WG-DROP: "
  }
}
```

Это фрагмент существующей конфигурации, не полный файл. Префикс — буквальная ASCII-строка до 64 bytes. ALLOW/AUDIT и посторонние program frames не превращаются в blocks. Отсутствие блокировок не доказывает восстановление соединения; recovery в этом срезе представлен fail2ban Unban.

Первый успешный запуск начинает с текущего конца, не рассылая старую историю. Live `/bans` при этом показывает уже действующие bans. После рестарта восстанавливаются cursor/offset. Выбранный источник сохраняется; обнаруженная смена target даёт постоянный coverage gap и требует явного выбора нового источника в конфигурации. Потерянный cursor восстанавливается по доступному времени; события, outbox и прогресс фиксируются одной транзакцией. Повторы сохраняют ID.

Чтение ограничено 64 записями за scan и 64 KiB на запись; файл может удерживать до четырёх поколений rotation. Действуют [ограничения восстановления SSH](ssh.md), включая недоказуемую полноту copytruncate и неизвестного порядка rotation. Symlinks, pipes и world-writable файлы не принимаются. Повреждённая запись не останавливает следующие записи или другой collector. Файловые события получают время наблюдения; journal сохраняет время записи. Сбор продолжается без Telegram, пока SQLite допускает приём.

### Read helper и права

Daemon остаётся непривилегированным. Helper работает отдельным root-процессом по одному запросу через systemd socket. Kernel UID peer должен совпасть с локальной root-owned policy; JSON не может передать UID, путь или команду. Helper выполняет только фиксированные fail2ban status/get-запросы. Ограничения: 32 jails, 1024 IP, 64 KiB ответа, 5 s клиента. Jail names ограничены ASCII letters/digits, `_`, `.` и `-`, первый символ — letter/digit. Неполнота показывается явно.

Ниже — дополнение проверенной ручной установки. Сначала сохраните прежнюю сборку, конфигурацию, состояние и units. Не перезаписывайте чужие policy или units.

```sh
sudo install -d -m 700 /etc/want-garde-helper
id -u want-garde | sudo tee /etc/want-garde-helper/daemon-uid >/dev/null
sudo chmod 600 /etc/want-garde-helper/daemon-uid
sudo install -m 644 installation/want-garde-helper.socket /etc/systemd/system/
sudo install -m 644 installation/want-garde-helper@.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now want-garde-helper.socket
```

Бинарник want-garde-helper должен находиться в той же проверенной immutable-сборке, что и daemon. Socket имеет владельца root, группу want-garde и mode 0660; policy принадлежит root. Helper не читает настройки, данные или backups Want Garde. Основной сервис не получает sudo, capabilities или доступ к управляющему socket fail2ban.

На Ubuntu файлы fail2ban.log/ufw.log обычно доступны группе adm. Согласованный drop-in текущей установки:

```ini
# /etc/systemd/system/want-garde.service.d/firewall-log-read.conf
[Service]
SupplementaryGroups=adm
```

Это даёт сервису чтение всех adm-readable журналов. Глобальное членство пользователя не меняется; существующая группа systemd-journal сохраняется. Не добавляйте отсутствующую или неподходящую группу на другой платформе без проверки источников. После установки drop-in нужны daemon-reload и restart want-garde.service. Настройки fail2ban и правила firewall автоматически не меняются.

### Совместимость и откат

SQLite user_version остаётся 1. Новые источники используют отдельные versioned meta-записи, SSH checkpoint и старые DTO сохраняют формат; SshHealth остаётся alias общей SourceHealth. Старый бинарник может продолжать доставку существующей очереди и игнорировать новые meta, но не собирает новые источники. Если в конфигурацию добавлены новые поля, перед возвратом старого бинарника восстановите совместимую копию конфигурации.

При откате остановите новый daemon и task-owned helper socket/instances, восстановите прежние current, units и drop-in, выполните daemon-reload и запустите прежний сервис. Сохраняйте новые данные и закрытый backup; не пересоздавайте SQLite. Полная Linux-матрица относится к #32. Native runtime подтверждается отдельно от локальных tests и синтетических parser fixtures.

## English

Want Garde persists fail2ban Ban, Restore Ban and Unban, UFW BLOCK, standard firewalld DROP/REJECT records, and explicitly configured nftables prefixes. Ban is critical, unban is info, and firewall block is warning. Evidence is bounded to jail, IP, backend, protocol, destination and port when available. Raw logs and packet tails are not stored.

### Reading and diagnostics

The owner can use `/bans`, `/bans --page 2`, and `/bans sshd 2` in a private chat. A summary page contains up to 10 jails; a jail page contains up to 20 IPs. Snapshots include collection time, current counts and an explicit partial flag. Partial or unavailable data never means zero bans. Groups, unknown users and callbacks do not invoke the helper. Unban belongs to #16.

`/bans` also exposes both sources, gaps, unsupported/malformed records and stable reason keys. Run `want-garde-daemon --firewall-status --config /etc/want-garde/config.json` as want-garde (for example with `sudo -u want-garde`); it reads JSON without the token or database writes. Exit codes are 0 for both sources healthy, 1 for incomplete/unavailable coverage, and 2 for configuration/database/diagnostic errors. A healthy reader proves neither firewall effectiveness nor observation of every blocked packet; source logging and rate limits constrain coverage.

### Sources and recovery

Existing configuration works without new fields. `fail2ban` accepts auto/journald/file modes, like `ssh`; `firewall` contains `source`, `backend` and optional `log_prefix`. Auto fail2ban reads the live logtarget/loglevel through the helper. File targets use that file; SYSLOG, SYSTEMD-JOURNAL, STDOUT, STDERR and SYSOUT use the journal. Logging must include NOTICE; WARNING/ERROR/CRITICAL omit bans. An unverified target does not pin a journal. Standard action and syslog formats are supported. The journal filter accepts root-origin records from a standard installation; SYSLOG_IDENTIFIER alone does not authenticate a producer.

Auto firewall selects active UFW with enabled logging, then firewalld with LogDenied enabled. UFW uses readable ufw.log or the kernel journal; firewalld uses the kernel journal. Plain nftables requires an explicit backend and literal prefix assigned only to DROP/REJECT rules. The log statement itself is non-terminating. NFLOG and audit formats are unsupported here. See the [nftables manual](https://netfilter.org/projects/nftables/manpage.html) and [firewalld LogDenied](https://firewalld.org/documentation/man-pages/firewalld.conf.html).

The JSON above is a fragment of installation configuration. Prefixes are literal ASCII strings up to 64 bytes. ALLOW/AUDIT and unrelated program frames do not become blocks. Silence does not imply connection recovery; fail2ban Unban is the recovery observation in this slice.

The first successful scan starts at the tail without historical notifications; live `/bans` still lists existing bans. Restart resumes the saved cursor/offset. Source selection persists; an observed target change creates a lasting coverage gap and requires an explicit configuration choice. Lost cursors recover available records by time. Events, outbox and progress commit atomically with stable replay IDs.

Scans read at most 64 records, each up to 64 KiB, and retain up to four file generations. [SSH recovery limits](ssh.md), including indistinguishable copytruncate and uncertain rotation order, also apply. Symlinks, pipes and world-writable files are rejected. Malformed records do not stop following records or other collectors. File events use observation time; journal events use record time. Collection continues offline while SQLite can admit data.

### Helper and installation

The daemon stays unprivileged. A separate root helper handles one typed request per systemd socket connection. Kernel peer UID must match root-owned local policy; client JSON cannot choose UID, paths or commands. Only fixed fail2ban status/get commands run. Bounds are 32 jails, 1024 IPs, 64 KiB per response and a 5-second client deadline. Jail names use ASCII letters/digits, `_`, `.` and `-`, starting with a letter/digit. Limits and per-jail errors produce explicit partial results.

Extend a verified manual installation with the commands and units above. Preserve the previous build, configuration, state and units first; do not overwrite unrelated policy or units. Install want-garde-helper in the same verified immutable build as the daemon. The socket is root:want-garde mode 0660; its UID policy is root-owned. The helper cannot read Want Garde configuration, data or backups. The daemon receives neither sudo/capabilities nor the fail2ban control socket.

The Ubuntu drop-in above grants the service access to every adm-readable log, while retaining systemd-journal. It does not change global account membership. Check actual permissions before selecting groups on another platform. Reload systemd and restart the daemon after applying the drop-in. Firewall rules and fail2ban configuration are never changed automatically.

### Compatibility and rollback

SQLite user_version stays 1. New collectors use independent versioned meta entries; SSH checkpoint/DTO shapes remain compatible, with SshHealth retained as an alias of shared SourceHealth. An older binary can deliver the existing queue and ignore new meta, but suspends the new collectors. Restore compatible configuration before using an older binary if new fields were added.

For rollback, stop the new daemon and installation-owned helper socket/instances, restore the previous current pointer, units and drop-in, reload systemd and restart the previous service. Retain new data and the private backup; never recreate SQLite. The complete Linux matrix belongs to #32. Native runtime evidence is separate from local tests and synthetic parser fixtures.

## Platform evidence / Проверка платформ

| Platform / Платформа                                   | Source / Источник                          | Evidence / Доказательства                                                                                                                    |
| ------------------------------------------------------ | ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Ubuntu 24.04 x86_64                                    | fail2ban, UFW                              | Native verification pending / Native-проверка ожидается                                                                                      |
| Ubuntu 24.04 x86_64                                    | nftables                                   | Synthetic packet fixtures; no live ruleset claim / Синтетические записи, без заявления о проверке действующих правил                         |
| firewalld                                              | Standard log formats / Стандартные форматы | Synthetic fixtures only; firewalld is absent on the current test host / Только синтетические примеры; на текущем хосте firewalld отсутствует |
| Debian, Rocky/AlmaLinux, Fedora, Arch, openSUSE; ARM64 | Planned / Планируется                      | Unverified; #32 / Не проверено; #32                                                                                                          |
