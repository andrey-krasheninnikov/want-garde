# Wire contracts / Wire-контракты

## Русский

Исходные Rust DTO находятся в crates/contracts. JSON Schema и TypeScript находятся в packages/contracts/generated. Команда cargo xtask contracts обновляет их; --check только сравнивает. Не редактируйте производные файлы вручную.

Wire version передаётся строкой "1" и не совпадает с SemVer продукта. Запросы принимают только известные поля и действия. Единственное начальное действие: server.status без параметров. Daemon #3 реализует это действие для локально привязанного владельца в личном Telegram-чате. CLI реализует локальную привязку; helper/MCP остаются каркасом.

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

Положительные и отрицательные примеры в fixtures/contracts являются синтетическими. Проверки сериализации, JSON Schema и TypeScript используют публичные контракты. Первичная авторизация, TOTP, SQLite, jobs persistence и транспортные adapters реализуются в следующих тикетах.

## English

Rust DTOs in crates/contracts own the contracts. JSON Schema and TypeScript live in packages/contracts/generated. Run cargo xtask contracts to update them; --check only compares. Never hand-edit derived files.

Wire version is the string "1", distinct from product SemVer. Requests accept only known fields/actions. The initial action is server.status with no arguments. The #3 daemon implements this action for the locally bound owner in a private Telegram chat. The CLI implements local binding; helper/MCP remain scaffolds.

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

Fixtures in fixtures/contracts are synthetic. Serialization, JSON Schema, and TypeScript checks use public contracts. Initial authorization, TOTP, SQLite, job persistence, and transport adapters arrive in later tickets.

## Host snapshot / Снимок хоста

The server_status result contains server_id, product_version, and a timestamped HostSnapshot. Each measurement is available with a typed value or unavailable with a stable reason_key. Byte counts use JSON numbers below 2^53; IDs remain strings. CPU is percent, load is the 1/5/15-minute tuple, uptime is seconds, memory/disk are total and available bytes.

Результат server_status содержит server_id, product_version и HostSnapshot со временем. Каждый показатель имеет типизированное значение available либо unavailable со стабильным reason_key. Объёмы передаются JSON-числами меньше 2^53; ID остаются строками. CPU измеряется в процентах, load содержит 1/5/15 минут, uptime передаётся в секундах, RAM/диск содержат общий и доступный объём в bytes.

By explicit pre-release decision, this extends wire version "1" without a version bump. Old strict scaffold readers cannot decode the new status result. Update Rust, JSON Schema, TypeScript, fixtures, and consumers together; compatibility with the earlier unpublished scaffold is not promised.

По согласованному решению до первого релиза wire version "1" развивается без смены номера. Старые строгие readers каркаса не прочитают новый результат статуса. Rust, JSON Schema, TypeScript, fixtures и потребители обновляются вместе; совместимость с прежним невыпущенным каркасом не обещается.
