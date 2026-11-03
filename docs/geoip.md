# Optional GeoIP / Необязательный GeoIP

## Русский

GeoIP выключен по умолчанию. Включение разрешает передачу публичного IP источника и IP вашего VPS внешнему провайдеру IPinfo. IPinfo Lite возвращает код страны, ASN и организацию без города. Эти сведения приблизительны и не меняют классификацию события. См. [документацию IPinfo](https://ipinfo.io/developers/lite-api) и [ADR-0011](adr/0011-geoip-enrichment.md).

### Настройка

Сохраните IPinfo token редактором в отдельный файл, например `/etc/want-garde/ipinfo-token`: владелец root, группа want-garde, права 0640. Родительский каталог не должен быть доступен группе или остальным на запись. Не передавайте token через аргументы команд, environment, Git или сообщения. Не используйте файл Telegram token.

В конфигурации задайте:

```json
"geoip": {
  "enabled": true,
  "token_file": "/etc/want-garde/ipinfo-token"
}
```

Проверьте конфигурацию и перезапустите daemon по [инструкции установки](installation.md). Настройки применяются после restart. Ошибка чтения token или отказ провайдера видны в GeoIP-диагностике; остальные мониторы продолжают работу. После исправления token выдерживается сохранённый cooldown.

Для отключения задайте `enabled: false` и перезапустите daemon. Отключённый GeoIP не читает файл token и не делает внешних запросов. Уже сохранённый контекст остаётся в истории до удаления события.

### Доставка и приватность

Обогащается только нормализованный IP источника SSH, fail2ban/firewall или x-ui. Destination endpoints, произвольный текст, private, loopback, link-local, multicast, documentation и остальные специальные адреса не отправляются провайдеру. IPv4-mapped IPv6 нормализуется перед проверкой. Скрытый или некорректный адрес не восстанавливается через GeoIP.

Публикация использует локальный кеш. На промахе событие доставляется сразу с отметкой ожидания lookup. Фоновый результат сохраняется для просмотра конкретного события и последующих попаданий в кеш. Исходное уведомление, включая его повторы, не меняется. Дополнительного сообщения нет. Классификация и исходный Event неизменяемы.

Состояния: `missing` с объяснением отсутствия, `retrieved` для полученного результата, `cached` для контекста из кеша на момент события. Поздний результат не меняет состояние уже отправленного текста. Raw JSON провайдера и token не сохраняются в событиях, кеше или диагностике. Контекст наследует приватность события; сейчас команды разрешены только владельцу в личных сообщениях.

### Лимиты и отказы

- Успешный кеш: 24 часа; отрицательный результат: 15 минут; максимум 1024 записи с вытеснением давно использованных.
- Очередь: 128 уникальных IP, ожидание до 10 минут; одинаковые IP объединяются. Переполнение пропускает обогащение и увеличивает счётчик.
- Один lookup одновременно, timeout 2 секунды, ответ до 16 KiB, максимум 30 запросов в минуту и 1000 за сутки UTC.
- Счётчики и cooldown сохраняются до HTTP и переживают restart. Перевод часов назад не сбрасывает бюджет. Прерванный lookup не повторяется автоматически.
- Отказы дают cooldown от 15 минут до 24 часов с учётом `Retry-After`. Прямых fallback-провайдеров и немедленных повторов нет.
- Очередь и кеш не обходят общую квоту SQLite. Контекст удаляется вместе с событием по действующим retention и quota. История до включения не отправляется провайдеру задним числом.

### Диагностика

`/geoip` показывает состояние, очередь, кеш, запросы за сутки, пропуски и cooldown. `/geoip EVENT_ID` показывает сохранённый контекст события. Это не команда lookup произвольного IP. Полный просмотр истории относится к #11.

Локальные команды читают базу без захвата writer, HTTP и token:

```sh
want-garde-daemon --geoip-status --config /etc/want-garde/config.json
want-garde-daemon --geoip-event EVENT_ID --config /etc/want-garde/config.json
```

Они выводят JSON: exit 0 для успешного чтения либо готового/отключённого GeoIP, exit 1 для внимания в статусе, exit 2 для ошибки входа или хранилища. При первом обновлении daemon создаёт дополнительные таблицы в schema 1. Старые конфигурации читаются с выключенным GeoIP. Откат использует прежние binaries и совместимую конфигурацию, сохраняя текущую базу и незавершённые доставки. Новые таблицы остаются; старый daemon их не исполняет.

## English

GeoIP is disabled by default. Enabling it permits disclosing the public source IP and your VPS's outbound IP to IPinfo. IPinfo Lite returns country code, ASN and organization without a city. These approximate details never change event classification. See [IPinfo documentation](https://ipinfo.io/developers/lite-api) and [ADR-0011](adr/0011-geoip-enrichment.md).

### Configuration

Use an editor to save the IPinfo token in a separate file, such as `/etc/want-garde/ipinfo-token`, owned by root, group want-garde, mode 0640. Its parent must not allow group or other writes. Never pass the token through command arguments, environment, Git or messages. Do not use the Telegram token file.

Set the configuration shown above. Check configuration and restart the daemon using the [installation guide](installation.md). Changes apply after restart. Token-read failures or provider rejection appear in GeoIP diagnostics; other monitors continue. Fixing the token does not bypass the persisted cooldown.

To disable enrichment, set `enabled: false` and restart. Disabled GeoIP neither reads the token nor makes external requests. Saved context remains in history until its event is deleted.

### Delivery and privacy

Only normalized source addresses from SSH, fail2ban/firewall and x-ui are eligible. Destination endpoints, arbitrary text, private, loopback, link-local, multicast, documentation and other special-purpose addresses stay local. IPv4-mapped IPv6 is normalized before filtering. GeoIP never reconstructs masked or invalid addresses.

Publication uses the local cache. A cache miss delivers the event immediately with lookup pending. Background results become available for that event and later cache hits. Original notification text, including retries, remains unchanged. No supplemental notification is sent. Classification and the original Event remain immutable.

States are `missing` with a reason, `retrieved` for a fetched result and `cached` for context reused at event time. Late results do not change an already sent message. Raw provider JSON and tokens never enter events, cache or diagnostics. Context inherits event privacy; current commands require the owner in a private chat.

### Limits and failures

- Positive cache TTL: 24 hours; negative TTL: 15 minutes; at most 1024 entries, evicting least recently used addresses.
- Queue: 128 distinct IPs with a 10-minute deadline and per-IP deduplication. Overflow skips enrichment and increments diagnostics.
- One lookup at a time, a 2-second timeout, at most 16 KiB per response, 30 requests per minute and 1000 per UTC day.
- Budgets and cooldown persist before HTTP and survive restart. Moving the clock backwards never resets the budget. Interrupted lookups are not automatically repeated.
- Failures impose a cooldown from 15 minutes to 24 hours, honoring `Retry-After`. There are no fallback providers or immediate retries.
- Queue and cache respect the shared SQLite quota. Context follows its event's retention and deletion. Existing history is not sent retrospectively after enabling enrichment.

### Diagnostics

`/geoip` reports state, queue, cache, daily requests, skips and cooldown. `/geoip EVENT_ID` reads saved event context. Arbitrary IP lookup is unavailable. Full history browsing belongs to #11.

The local commands shown above read the database without claiming the writer, making HTTP requests or reading the token. They emit JSON: exit 0 for a successful event read or ready/disabled status, exit 1 for status requiring attention, and exit 2 for input or storage failure.

The daemon adds tables to schema 1 on first startup. Old configurations default to disabled GeoIP. Rollback uses previous binaries and a compatible configuration while preserving the current database and unfinished deliveries. New tables remain; the old daemon does not execute their pending work.
