# Want Garde

Единый контекст мониторинга и управления одной установкой Want Garde. / One context covers monitoring and administration of a Want Garde installation.

## Language / Язык

**Installation / Установка**

Независимая установка на одном VPS с собственной identity сервера и Telegram-ботом. / An independent installation on one VPS with its own server identity and Telegram bot.

**Event / Событие**

Неизменяемое наблюдение об активности или состоянии VPS с источником, временем, классификацией и evidence. / An immutable observation of VPS activity or state with source, time, classification, and evidence.
_Avoid_: Notification / Уведомление.

**Evidence / Подтверждающие данные**

Ограниченные данные, объясняющие наблюдение и его классификацию, без секретов процессов. / Bounded data explaining an observation and its classification without process secrets.

**Incident / Инцидент**

Связанные события и хронология их разбора; инцидент может быть открытым, подтверждённым или закрытым. / Related events and their investigation timeline; an incident can be open, acknowledged, or closed.

**Principal / Субъект доступа**

Подтверждённая identity пользователя или интеграции, обращающейся к установке. / An authenticated identity of a user or integration accessing the installation.
_Avoid_: User / Пользователь, когда речь также об интеграциях / when integrations are also included.

**Capability / Разрешение на функцию**

Явное разрешение на конкретную функцию Want Garde. / Explicit permission for a particular Want Garde function.
_Avoid_: Role / Роль.

**Scope / Область разрешения**

Ограничение capability конкретным сервером, ресурсом и контекстом обращения. / Restriction of a capability to a particular server, resource, and request context.

**Operation request / Запрос операции**

Типизированное намерение выполнить действие; само по себе не даёт полномочий. / A typed intent to perform an action; it grants no authority by itself.

**Approval / Подтверждение**

Одноразовое согласие с точными параметрами действия, привязанное к инициатору и серверу. / Single-use consent to exact action parameters, bound to the initiator and server.
_Avoid_: Permission / Разрешение.

**Job / Задание**

Наблюдаемое выполнение запроса операции с состоянием и результатом. / An observable execution of an operation request with state and outcome.
_Avoid_: Operation request / Запрос операции, когда речь об исполнении / when referring to execution.

**Reconciliation / Сверка результата**

Проверка фактического результата после неоднозначного ответа до любого повтора действия. / Inspection of the actual outcome after an ambiguous response, before any retry.

**Action audit / Аудит действий**

Сохраняемая история запроса, решения о доступе, подтверждения и результата действия. / Persisted history of a request, access decision, approval, and action outcome.

**Outbox / Очередь доставок**

Сохраняемая до отправки очередь уведомлений с независимым состоянием каждого получателя. / A notification queue persisted before transmission, with independent state for each recipient.

**Collector / Источник наблюдений**

Адаптер наблюдения за выбранным источником состояния или активности VPS. / An observation adapter for a selected source of VPS state or activity.

**Delivery / Доставка**

Намерение отправить одно сообщение конкретному разрешённому получателю, со стабильным идентификатором и сохраняемым итогом всех повторов. / One intent to deliver a message to a particular permitted recipient, with a stable identifier and a persisted outcome across retries.
_Avoid_: Event / Событие, когда речь об отправке сообщения / when referring to message transmission.

**Dead letter / Неисправимая доставка**

Завершённая доставка, которая не получила успешного подтверждения из-за постоянной ошибки, истечения срока или исчерпания повторов. / A terminal delivery without successful acknowledgement because of a permanent error, expiration, or exhausted retries.

**Coverage / Покрытие наблюдения**

Наблюдаемая доступность источников; отсутствие данных не доказывает исправность VPS. / Observable source availability; absence of data does not prove VPS health.

**Helper / Административный исполнитель**

Ограниченный привилегированный исполнитель разрешённых административных действий Want Garde. / A restricted privileged executor of permitted Want Garde administrative actions.

**Runner / Исполнитель команд**

Отдельная непривилегированная identity выполнения shell-заданий. / A separate unprivileged identity for shell jobs.

**Host snapshot / Снимок хоста**

Состояние одной установки на момент измерения, включающее доступные показатели и явные сведения о недоступных показателях. / The state of one installation at a measurement time, including available measurements and explicit information about unavailable measurements.

**Coverage gap / Пропуск наблюдения**

Интервал, для которого установка не может подтвердить полноту собранных наблюдений. Восстановление доступности источника не устраняет уже возникший пропуск. / An interval for which the installation cannot establish complete observation coverage. Restoring source availability does not erase an earlier gap.

**Jail / Jail fail2ban**

Именованный набор правил обнаружения и блокировок fail2ban с текущим списком заблокированных адресов. Просмотр jail не даёт разрешения на unban. / A named fail2ban detection and banning configuration with a current list of banned addresses. Reading a jail grants no unban authority.

**Resource threshold / Порог ресурса**

Проверяемая граница показателя хоста с сохраняемым состоянием warning/critical и отдельным восстановлением. / A host measurement limit with durable warning/critical state and separate recovery.

**Listener baseline / Baseline listeners**

Сохранённый набор ожидаемых listening endpoints одной установки, с которым сравниваются наблюдения. / A saved set of expected listening endpoints for one installation, used to compare observations.
_Avoid_: Последний снимок / Latest snapshot.

**Panel snapshot / Снимок панели**

Нормализованное наблюдение конфигурации x-ui/3x-ui и метаданных клиентов с fingerprints конфигурационных значений и явной доступностью данных. / A normalized observation of x-ui/3x-ui configuration and client metadata, with configuration fingerprints and explicit data availability.
