# Wire contracts / Wire-контракты

## Русский

Исходные Rust DTO находятся в crates/contracts. JSON Schema и TypeScript находятся в packages/contracts/generated. Команда cargo xtask contracts обновляет их; --check только сравнивает. Не редактируйте производные файлы вручную.

Wire version передаётся строкой "1" и не совпадает с SemVer продукта. Запросы принимают только известные поля и действия. Действия чтения: server.status и fail2ban.status без параметров. Daemon #3 реализует это действие для локально привязанного владельца в личном Telegram-чате. CLI реализует локальную привязку; helper предоставляет отдельный ограниченный read-протокол; MCP остаётся каркасом.

ID являются непустыми ASCII-строками до 128 символов: первый символ буква или цифра, остальные также допускают точку, подчёркивание, двоеточие и дефис. Это сохраняет точность JavaScript. Telegram chat ID в Scope передаются отдельными десятичными строками; проверка подлинности и допустимости выполняется прикладной политикой в #15. Время событий и инцидентов имеет формат RFC 3339.

| Contract         | Значение                                                                                                                                    |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Event            | server_id, observed_at, source, classification, evidence; ID стабилен при повторной доставке.                                               |
| Incident         | Ссылки event_ids, severity и состояние open/acknowledged/closed. Закрытое состояние содержит by, at и reason.                               |
| Grant            | principal_id, scope сервера/ресурсов/чатов и явные capabilities; это данные доверенного хранилища, а не клиентское подтверждение доступа.   |
| OperationRequest | version, request_id, server_id и типизированное action. Запрос ограничен 64 KiB UTF-8.                                                      |
| Job              | ID запроса, сервера и задания; состояния awaiting_approval, queued, running, succeeded, failed, cancelled, timed_out, needs_reconciliation. |
| ContractError    | Стабильный code и message_key; входные параметры и секреты не отражаются в ошибке декодирования.                                            |

State/result являются discriminated unions. Success содержит типизированный результат; failure содержит ошибку; неоднозначный эффект требует needs_reconciliation. Прикладная реализация проверяет допустимость переходов, ссылки, непустую причину закрытия, quotas, права и redaction. DTO-проверка не заменяет эти политики.

Перед новой функцией добавьте её typed action/result и необходимые capabilities. Изменения текущих строгих DTO требуют анализа совместимости и явного решения о wire version. Старый reader не обязан принимать новые поля. TypeScript помогает при компиляции, но не проверяет сетевые данные во время исполнения; сервер всегда декодирует и проверяет ввод.

Положительные и отрицательные примеры в fixtures/contracts являются синтетическими. Проверки сериализации, JSON Schema и TypeScript используют публичные контракты. Приватный доступ владельца, SQLite событий/outbox и Telegram transport реализованы в #3/#4; TOTP и jobs persistence следуют в профильных тикетах.

## English

Rust DTOs in crates/contracts own the contracts. JSON Schema and TypeScript live in packages/contracts/generated. Run cargo xtask contracts to update them; --check only compares. Never hand-edit derived files.

Wire version is the string "1", distinct from product SemVer. Requests accept only known fields/actions. Read actions are server.status and fail2ban.status without arguments. The #3 daemon implements this action for the locally bound owner in a private Telegram chat. The CLI implements local binding; the helper exposes a separate restricted read protocol; MCP remains a scaffold.

IDs are nonempty ASCII strings up to 128 characters: the first character is alphanumeric; remaining characters additionally allow dot, underscore, colon, and hyphen. This preserves JavaScript precision. Telegram chat IDs in Scope are separate decimal strings; application policy in #15 validates authenticity and permission. Event/incident timestamps use RFC 3339.

| Contract         | Meaning                                                                                                                                |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Event            | server_id, observed_at, source, classification, evidence; the ID remains stable on redelivery.                                         |
| Incident         | event_ids, severity, and open/acknowledged/closed state. A closed state includes by, at, and reason.                                   |
| Grant            | principal_id, server/resource/chat scope, and explicit capabilities; trusted-store data, not a client claim of access.                 |
| OperationRequest | version, request_id, server_id, and typed action; input is bounded to 64 KiB UTF-8.                                                    |
| Job              | Request/server/job identity; awaiting_approval, queued, running, succeeded, failed, cancelled, timed_out, needs_reconciliation states. |
| ContractError    | Stable code and message_key; decoding errors never echo input parameters or secrets.                                                   |

State/result are discriminated unions. Success carries a typed result; failure carries an error; ambiguous effects require needs_reconciliation. Application implementation validates state transitions, references, a nonempty closure reason, quotas, permissions, and redaction. DTO validation does not replace these policies.

Before adding a feature, add its typed action/result and capabilities. Changes to the current strict DTOs require compatibility analysis and an explicit wire-version decision. Old readers need not accept new fields. TypeScript provides compile-time help, not network-data validation; the server always decodes and validates input.

Fixtures in fixtures/contracts are synthetic. Serialization, JSON Schema, and TypeScript checks use public contracts. Private owner access, SQLite events/outbox, and Telegram transport are implemented in #3/#4; TOTP and job persistence follow in their feature issues.

## Host snapshot / Снимок хоста

The server_status result contains server_id, product_version, and a timestamped HostSnapshot. Each measurement is available with a typed value or unavailable with a stable reason_key. Byte counts use JSON numbers below 2^53; IDs remain strings. CPU is percent, load is the 1/5/15-minute tuple, uptime is seconds, memory/disk are total and available bytes.

Результат server_status содержит server_id, product_version и HostSnapshot со временем. Каждый показатель имеет типизированное значение available либо unavailable со стабильным reason_key. Объёмы передаются JSON-числами меньше 2^53; ID остаются строками. CPU измеряется в процентах, load содержит 1/5/15 минут, uptime передаётся в секундах, RAM/диск содержат общий и доступный объём в bytes.

By explicit pre-release decision, this extends wire version "1" without a version bump. Old strict scaffold readers cannot decode the new status result. Update Rust, JSON Schema, TypeScript, fixtures, and consumers together; compatibility with the earlier unpublished scaffold is not promised.

По согласованному решению до первого релиза wire version "1" развивается без смены номера. Старые строгие readers каркаса не прочитают новый результат статуса. Rust, JSON Schema, TypeScript, fixtures и потребители обновляются вместе; совместимость с прежним невыпущенным каркасом не обещается.

## Delivery diagnostics / Диагностика доставки

DeliveryHealth is a separate additive wire-version "1" DTO. It contains queue counts, oldest pending time, rate-limited recipient count, pruning count, and up to 20 DeliveryProblem entries. Problems carry stable string IDs, delivery state, attempt count, reason_key, uncertainty, and update time. Message contents, recipient IDs, and tokens are excluded. The daemon enforces response bounds; generated types provide no authorization. The existing server.status action and result are unchanged.

DeliveryHealth добавлен отдельным DTO wire version "1". Он содержит счётчики очереди, время самой старой ожидающей доставки, число получателей на паузе, счётчик очистки и до 20 записей DeliveryProblem. Проблемы содержат стабильные строковые ID, состояние доставки, число попыток, reason_key, признак неоднозначности и время изменения. Тексты сообщений, ID получателей и токены исключены. Daemon ограничивает размер результата; производные типы не дают полномочий. Действие server.status и его результат сохраняют прежний формат.

## SSH health

`SshHealth` описывает доступность выбранного SSH-источника, начало покрытия, время успешной проверки, последнюю запись, неподдержанные записи и сохраняемые пропуски. Состояния: `starting`, `healthy`, `degraded`, `unavailable`. Контракт не содержит сырых журналов или токена.

`SshHealth` describes selected-source availability, coverage start, successful check and record times, unsupported records, and persistent gaps. States are `starting`, `healthy`, `degraded`, and `unavailable`. It contains no raw journal or token.

## Firewall and bans / Firewall и bans

`BansSnapshot` содержит время, текущие jails/counts/IP, partial и ограниченные сведения о logtarget/loglevel. Неизвестный count представлен null. `FirewallHealth` содержит здоровье двух независимых readers и выбранный backend. `SourceHealth` обобщает прежний `SshHealth`, чей JSON-формат сохраняется; Rust/TypeScript alias остаётся доступным. Новые action/result и `bans.read` добавляются в pre-release wire version "1". [Границы доступа и runtime](firewall.md).

`BansSnapshot` contains time, current jails/counts/IPs, partial, and bounded logtarget/loglevel metadata. An unknown count is null. `FirewallHealth` contains two independent reader states and the selected backend. `SourceHealth` generalizes `SshHealth` while preserving its JSON shape and Rust/TypeScript alias. New action/result and `bans.read` extend pre-release wire version "1". See [access and runtime boundaries](firewall.md).

## Host health extensions / Расширения состояния хоста

HostSnapshot adds `cpu_count`, `load_per_core`, `disks`, and `reboot_required`. Missing new measurements decode as unavailable and missing disks as an empty list. Existing fields remain; root_disk is unavailable when `/` is not configured. DiskUsage distinguishes occupied bytes from user-available bytes; InodeUsage contains total and available counts. HostHealth exposes the latest snapshot, measurement time, stale status, and active resource or coverage alerts. Exported wire version remains `1`; the unreleased components update together. Older strict decoders must use their matching component version. SQLite remains schema version `1`; rollback preserves new host metadata without replaying it.

HostSnapshot добавляет `cpu_count`, `load_per_core`, `disks` и `reboot_required`. Отсутствующие новые измерения читаются как недоступные, отсутствующий список дисков как пустой. Старые поля сохраняются; root_disk недоступен, когда `/` не включён. DiskUsage различает занятые и доступные пользователю байты; InodeUsage содержит общее и доступное число inode. HostHealth показывает последний снимок, время измерения, устаревание и активные тревоги ресурсов или покрытия. Wire version остаётся `1`; компоненты неопубликованной версии обновляются вместе. Старые строгие decoders используют соответствующую версию компонентов. SQLite сохраняет schema version `1`; откат удерживает новые host metadata без их повторного исполнения.

## Services and listeners / Службы и listeners

`services.status` and `ports.status` add typed read actions/results and corresponding read capabilities. `ServicesHealth`, `PortsHealth`, `BaselinePreview` and the separate `ListenerReadResponse` are exported with their schemas and TypeScript. Baseline acceptance is a local daemon command, not a remote action. Existing components must use the same unreleased product revision; wire version remains 1.

`services.status` и `ports.status` добавляют типизированные read actions/results и соответствующие read capabilities. `ServicesHealth`, `PortsHealth`, `BaselinePreview` и отдельный `ListenerReadResponse` экспортируются вместе со schemas и TypeScript. Принятие baseline является локальной командой daemon, не удалённым action. Компоненты используют одну unreleased-ревизию продукта; wire version остаётся 1.

## x-ui observation / Наблюдение x-ui

`xui.status` и `xui.read` описывают просмотр сохранённого состояния; разрешений на изменение панели нет. `XuiHealth` отделяет здоровье источников от состояния соединений. Закрытый `XuiReadRequest` допускает только snapshot либо logs с checkpoint; `XuiReadResponse` содержит нормализованные данные без сырых журналов и секретов. / `xui.status` and `xui.read` describe saved-state inspection without panel mutations. `XuiHealth` separates source health from connectivity. The closed `XuiReadRequest` accepts only snapshot or checkpointed logs; `XuiReadResponse` contains normalized data without raw logs or secrets.
