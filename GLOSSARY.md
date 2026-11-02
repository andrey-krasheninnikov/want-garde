# Domain glossary / Доменный словарь

One context covers the complete Want Garde product. / Единый контекст охватывает весь Want Garde.

| Term | Русский | English |
| --- | --- | --- |
| Installation | Независимая установка на одном VPS со своим server ID и bot token. | Independent installation on one VPS with its own server ID and bot token. |
| Event | Неизменяемое наблюдение с источником, временем, категорией, важностью и evidence. | Immutable observation with source, time, category, severity, and evidence. |
| Evidence | Ограниченные подтверждающие данные без секретов, argv и environment процессов. | Bounded supporting data without secrets, process argv, or process environment. |
| Incident | Связанные события и хронология разбора; состояния open, acknowledged, closed. | Related events and investigation timeline; open, acknowledged, and closed states. |
| Principal | Подтверждённая интерфейсом identity пользователя или интеграции. | User or integration identity authenticated by its interface. |
| Capability | Явное разрешение на функцию; статус администратора группы его не заменяет. | Explicit permission for a function; group-administrator status is not a substitute. |
| Scope | Ограничение разрешения сервером, ресурсом и допустимым контекстом обращения. | Restriction of a permission to a server, resource, and permitted request context. |
| Operation request | Типизированное намерение выполнить действие; само по себе не даёт полномочий. | Typed intent to perform an action; it grants no authority by itself. |
| Approval | Одноразовое согласие с точным preview, привязанное к инициатору, серверу и параметрам. | Single-use consent to an exact preview, bound to initiator, server, and parameters. |
| Job | Сохраняемое выполнение запроса с наблюдаемым состоянием и результатом. | Persisted execution of a request with observable state and outcome. |
| Reconciliation | Проверка фактического результата после неоднозначного ответа до любого повтора. | Inspection of the actual outcome after an ambiguous response, before any retry. |
| Action audit | Сохраняемая запись запроса, решения о доступе, подтверждения и результата. | Persisted record of a request, access decision, approval, and outcome. |
| Outbox | Сохраняемая до отправки очередь доставок с независимым состоянием каждого получателя. | Delivery queue persisted before sending, with independent state for each recipient. |
| Collector | Адаптер чтения источника, сохраняющий cursor или offset для восстановления. | Source-reading adapter that persists a cursor or offset for recovery. |
| Coverage | Наблюдаемая доступность источников; отсутствие данных не доказывает исправность. | Observable source availability; absence of data does not prove health. |
| Helper | Ограниченный привилегированный исполнитель типизированных административных действий. | Restricted privileged executor of typed administrative actions. |
| Runner | Отдельный непривилегированный Linux-пользователь для shell-заданий. | Separate unprivileged Linux identity for shell jobs. |
| Contract version | Версия wire-формата, отличная от общей SemVer-версии продукта. | Wire-format version, distinct from the shared product SemVer version. |
