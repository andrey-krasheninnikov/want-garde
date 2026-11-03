# ADR-0011: Optional GeoIP context / Необязательный GeoIP-контекст

Status / Статус: Accepted / Принято.

## Decision / Решение

GeoIP is disabled by default. Explicit local configuration permits disclosing normalized public source IPs to IPinfo Lite over HTTPS. The token lives in a separate restricted file and travels only in an Authorization header. Country code, ASN and organization are context, not evidence of identity or malicious activity.

GeoIP выключен по умолчанию. Явная локальная настройка разрешает передачу нормализованных публичных IP источника в IPinfo Lite через HTTPS. Token находится в отдельном защищённом файле и передаётся только в Authorization header. Код страны, ASN и организация дают контекст, но не доказывают личность или вредоносность активности.

Event publication uses only the local cache. A bounded background worker performs external lookups without delaying collection or delivery. A cache miss leaves the original notification unchanged; a later result becomes available through saved event context and the cache. No supplemental notification is sent. Events, classifications and retry text remain immutable.

Публикация события использует только локальный кеш. Ограниченный фоновый worker выполняет внешние lookup без задержки сбора и доставки. Промах кеша не меняет исходное уведомление; поздний результат доступен через сохранённый контекст события и кеш. Дополнительного уведомления нет. События, классификация и текст повторных доставок неизменяемы.

## Consequences / Последствия

Cache entries, pending lookups and request budgets survive restarts. Additive SQLite storage preserves the existing schema-1 event and delivery tables; rollback retains new metadata without replaying it. Context follows event retention and access rules. Monitoring continues when enrichment is disabled, unavailable or at capacity. History browsing beyond a single event remains separate work.

Кеш, ожидающие lookup и бюджеты запросов переживают restart. Аддитивное хранение SQLite сохраняет существующие таблицы событий и доставок schema 1; откат удерживает новые metadata без повторного исполнения. Контекст наследует retention и доступ события. Мониторинг продолжается при отключённом, недоступном или перегруженном обогащении. Просмотр истории за пределами одного события остаётся отдельной задачей.
