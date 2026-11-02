# SSH monitoring / Мониторинг SSH

## Русский

Want Garde читает SSH-события и сохраняет позицию вместе с событием и доставкой. Первый успешный запуск начинает с текущего конца журнала. Старая история не отправляется. Начало покрытия видно в диагностике. Сбор работает при недоступном Telegram, пока позволяют квоты существующей очереди.

Необязательный блок `ssh` в конфигурации принимает `{"mode":"auto"}`, `{"mode":"journald"}` или `{"mode":"file","path":"/var/log/auth.log"}`. Без блока используется `auto`: journald, затем доступный `/var/log/auth.log`, затем `/var/log/secure`. Выбирается один источник, и выбор сохраняется. Временная ошибка не переключает источник. Явная смена режима или пути начинает новое покрытие с конца и фиксирует пропуск.

Уведомления покрывают успешный вход, ошибку входа, invalid user, logout, pre-auth close и scanner noise. Поддерживаются основные форматы OpenSSH, IPv4/IPv6, syslog и сообщения journald от `sshd`, `sshd-session`, `sshd-auth`. Вход и logout имеют важность Info; остальные события Warning. Обогащение GeoIP и агрегация шума относятся к отдельным задачам.

### Права чтения

Поставляемый systemd unit запускает службу без root и capabilities и задаёт `SupplementaryGroups=systemd-journal`. Это даёт службе чтение всех системных журналов; сборщик выбирает SSH-записи по процессу или syslog identifier. Членство пользователя в системе не меняется. Журнальная запись является наблюдением и не предоставляет разрешений на операции.

Файловый режим требует отдельных прав чтения файла и ротаций. Daemon не меняет ACL, logrotate или journald. Настройте сохранение прав при ротации локально. Если группа `systemd-journal` отсутствует и используется только файл, очистите `SupplementaryGroups` в локальном systemd override. Не запускайте daemon от root и не выдавайте произвольный sudo.

### Восстановление и ограничения

Journald cursor проверяется на точное совпадение. При его потере чтение продолжается с доступных записей около последнего сохранённого времени с перекрытием в одну секунду, но не раньше начала покрытия. Пропуск сохраняется в диагностике. Изменение системного времени может сделать восстановление неполным. При обычном чтении порядок задаёт cursor.

Для файла сохраняются device, inode, поколение, byte offset и SHA-256 последних 64 байт перед позицией. Несжатая ротация ищется по inode. Открытые старые файлы продолжают читаться, включая поздние записи. Хранятся до четырёх поколений; поиск ограничен 129 записями каталога за проход. Нехватка сохранённых поколений или исчезновение старого inode фиксируется как пропуск.

Truncate и несовпадение контрольного фрагмента начинают новое поколение с нуля. Copytruncate, удаление и сжатие файлов могут уничтожить непрочитанные данные. Восстановление этих данных не гарантируется. Пограничные случаи copytruncate без наблюдаемого изменения содержимого могут остаться неразличимыми для файлового чтения.

Один проход обрабатывает до 64 записей или фрагментов. Запись ограничена 64 KiB; незавершённая обычная строка ждёт продолжения, слишком длинная пропускается ограниченными фрагментами. Неподдержанные SSH-записи учитываются счётчиком. Другие программы из `auth.log` не создают SSH-событий. Journald subprocess ограничен по выводу и времени; pager и shell не используются.

Сохраняются разобранные пользователь, IP, порт, метод и тип сканирования. Произвольные хвосты, ключи и исходные строки целиком в данные Want Garde не копируются. ID события остаётся стабильным при повторном чтении той же записи; существующая доставка сохраняет текст и язык. Дубликаты после неоднозначного подтверждения Telegram остаются возможны в рамках семантики #4. Подавление повторов опирается на сохраняемые события и текущие ограничения хранения.

### Диагностика и совместимость

Владелец в личном чате использует `/ssh`. Локальная команда `want-garde-daemon --ssh-status --config /etc/want-garde/config.json` запускается от пользователя службы, работает без токена и сети и не меняет базу. JSON содержит `SshHealth`: выбранный источник, состояние, начало покрытия, время проверки, последнюю запись, счётчики и причины пропусков. Отсутствие новых SSH-записей само по себе не считается отказом.

Exit codes: `0` healthy, `1` starting/degraded/unavailable, `2` ошибка конфигурации или хранилища. После 15 секунд без продвижения позиции очередная успешная проверка сохраняет heartbeat; после 60 секунд без успешной проверки состояние считается устаревшим. Последний пропуск сохраняется после восстановления доступности, поэтому degraded не означает, что чтение остановлено. Проверяйте также `/delivery`.

SQLite хранит версионированное состояние в `meta` под ключом `ssh.state`; основная схема остаётся v1. Неизвестное или повреждённое расширение не пересоздаётся. Версия #4 игнорирует расширение и может продолжить доставку, но не сбор SSH. Для возврата старого бинарника восстановите совместимую конфигурацию из резервной копии: #4 не принимает новый блок `ssh`. При обновлении сохраняйте базу, настройки владельца и языка; не удаляйте состояние ради устранения предупреждения.

## English

Want Garde checkpoints SSH input together with its event and delivery intent. The first successful start uses the current tail and sends no old history. Diagnostics show the coverage start. Collection continues while Telegram is unavailable, within the existing queue quotas.

The optional `ssh` block accepts `{"mode":"auto"}`, `{"mode":"journald"}`, or `{"mode":"file","path":"/var/log/auth.log"}`. Omission means `auto`: journald, then readable `/var/log/auth.log`, then `/var/log/secure`. One selected source is persisted. A temporary failure never switches it. An explicit mode/path change starts new coverage at the tail and records a gap.

Events cover accepted/failed login, invalid user, logout, pre-auth close, and scanner noise. Common OpenSSH formats, IPv4/IPv6, syslog, and journal records from `sshd`, `sshd-session`, and `sshd-auth` are handled. Login/logout use Info; the other events use Warning. GeoIP and noise aggregation are separate work.

### Read access

The packaged systemd unit runs without root or capabilities and sets `SupplementaryGroups=systemd-journal`. This grants the service access to all system journals; the collector selects SSH by process or syslog identifier. It does not change account membership. A journal record is an observation, never permission to perform an operation.

File mode requires read access to the file and rotations. The daemon changes no ACL, logrotate, or journald settings. Arrange rotation permissions locally. When the journal group does not exist and only file input is used, clear `SupplementaryGroups` in a local systemd override. Do not use a root daemon or unrestricted sudo.

### Recovery and limits

The journal cursor is checked for an exact match. A missing cursor resumes available records around the last saved time with a one-second overlap, never before coverage began, and records a persistent gap. System-clock changes can make recovery incomplete. Normal ingestion follows the cursor.

File progress stores device, inode, generation, byte offset, and SHA-256 of the preceding 64 bytes. Uncompressed rotations are located by inode. Open old files remain readable for late writes. At most four generations and 129 directory entries per search are handled. Missing generations or an unavailable old inode record a gap.

Truncation or a changed anchor starts a generation at zero. Copytruncate, deletion, and compression can destroy unread data; recovery cannot guarantee it. Copytruncate cases without an observable content change can remain indistinguishable to a file reader.

A scan processes up to 64 records or fragments. Records are bounded at 64 KiB. Ordinary incomplete lines wait for completion; oversized lines are skipped in bounded fragments. Unsupported SSH records increment a counter. Other programs in `auth.log` do not create SSH events. Journal subprocess output and duration are bounded, with no shell or pager.

Stored data contains parsed user, IP, port, method, and scanner kind. Arbitrary tails, keys, and complete raw lines are not copied into Want Garde data. Event identity is stable when the same record is reread; an existing delivery retains its text and language. Ambiguous Telegram acknowledgements can still cause duplicates under #4 semantics. Replay suppression depends on retained events and current storage limits.

### Diagnostics and compatibility

The owner uses `/ssh` in a private chat. Run `want-garde-daemon --ssh-status --config /etc/want-garde/config.json` as the service user for read-only JSON without a token or network. `SshHealth` shows source, state, coverage start, check/record times, and gap/unsupported-record details. Lack of new SSH activity is not itself failure.

Exit codes are `0` healthy, `1` starting/degraded/unavailable, and `2` configuration/storage error. After 15 seconds without checkpoint progress, the next successful check persists a heartbeat; 60 seconds without a successful check is stale. Historical gaps remain visible after access recovers, so degraded does not mean ingestion stopped. Check `/delivery` as well.

Versioned state lives in the SQLite `meta` key `ssh.state`; the main schema remains v1. Unknown or damaged extensions are rejected without reconstruction. Version #4 ignores the extension and can continue delivery while SSH collection stops. Restore a compatible configuration backup when reverting the binary: #4 rejects the new `ssh` block. Preserve the database, owner settings, and language during updates; do not delete state to clear a warning.
