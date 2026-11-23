# Политика уведомлений / Notification policy

## Русский

Секция **notification_policy** включает правила доставки. Событие всегда остаётся в истории.
Для каждого конкретного **classification.rule_id** задайте **enabled**, **severity**
(важность доставки), **min_severity**, **immediate**, **aggregate** и **dedupe_seconds**.
При отсутствии правила применяется **default_rule**. Без секции действует обычная доставка.

**default_preferences** задаёт порог, часовой пояс IANA и тихие часы для получателей.
Запись в **recipients** с ключом chat ID заменяет набор предпочтений получателя.
Список получателей задаётся отдельно в **notifications**. Политика сама не подписывает чат.

Интервалы **quiet_hours** имеют формат HH:MM-HH:MM, включают начало и исключают конец.
Окно может пересекать полночь. **critical_bypasses_quiet** явно разрешает обход для critical.
Обход по умолчанию выключен. Очередь перепроверяет тишину перед отправкой.
Дедупликация работает отдельно по получателю и сохраняется после перезапуска. Повторы не подавляют
вклад событий в оперативную сводку. Подавление immediate не удаляет исходное событие.

**daily** включает ежедневную сводку. **at** по умолчанию 14:00 в указанном часовом поясе.
**hours** задаёт окно подсчёта (24 часа по умолчанию). В пропущенный при переводе часов
момент используется первое доступное время того же дня. При повторённом времени выбирается
первое наступление. Сводка создаётся один раз для даты и получателя. Очередь принимает
пропущенные сводки в пределах срока хранения.

**live** включает оперативную сводку для событий с **aggregate**. По умолчанию окно 300 секунд
и порог три события. Готовые сводки ждут окончания тишины. Маршруты **summary.daily** и
**summary.live** можно указать для тем. В личном чате **/notifications** показывает политику
и запланированное время. Доступность Telegram влияет на фактическую отправку.

## English

The optional **notification_policy** section enables delivery rules. Events remain in history.
Rules match a specific **classification.rule_id** and configure **enabled**, **severity**
(the delivery severity), **min_severity**, **immediate**, **aggregate**, and **dedupe_seconds**.
Unmatched rules use **default_rule**. With the section absent, regular delivery applies.

**default_preferences** supplies the threshold, IANA time zone, and quiet hours for recipients.
An entry in **recipients** keyed by chat ID replaces that recipient's preference set.
The **notifications** section controls recipients. A policy entry does not subscribe a chat.

**quiet_hours** takes HH:MM-HH:MM windows with inclusive starts and exclusive ends.
Windows can cross midnight. **critical_bypasses_quiet** enables the critical bypass explicitly.
The bypass defaults to off. The queue checks quiet hours again before sending.
Deduplication applies per recipient and survives restart. Duplicate alerts still count toward
live summaries. Suppressing an immediate notification preserves the original event.

**daily** enables a daily summary. **at** defaults to 14:00 in the configured time zone.
**hours** sets the lookback, defaulting to 24. If a clock change skips the configured minute,
the summary is due at the first valid minute on that date. The earlier occurrence is used
when the minute repeats. A summary is queued once per date and recipient. Missed summaries
catch up within the configured retention period.

**live** enables live summaries for rules with **aggregate**. The defaults are a 300-second
window and a minimum of three events. Summaries wait until quiet hours end. Use **summary.daily**
and **summary.live** as event types in forum rules. The private **/notifications** command
reports the policy and the scheduled time. Actual sending depends on Telegram availability.

```json
{
  "notification_policy": {
    "default_preferences": {
      "time_zone": "Europe/Moscow",
      "quiet_hours": ["22:00-07:00"],
      "critical_bypasses_quiet": true,
      "daily": { "at": "14:00", "hours": 24 },
      "live": { "window_seconds": 300, "minimum_events": 3 }
    },
    "rules": {
      "ssh.failed": { "immediate": false, "aggregate": true }
    }
  }
}
```
