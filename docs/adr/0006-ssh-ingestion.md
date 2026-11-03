# ADR-0006: SSH checkpoints / Позиции чтения SSH

Status / Статус: Accepted / Принято.

Событие SSH, доставка и позиция чтения фиксируются одной SQLite-транзакцией. Первый запуск начинает с конца; выбранный источник сохраняется, а потеря позиции даёт явный пропуск и восстановление доступных записей. Это исключает отдельное подтверждение чтения до сохранения события. Состояние хранится как ограниченное версионированное расширение `meta`, без изменения существующих таблиц и основной версии схемы. Возврат к #4 сохраняет доставку и приостанавливает сбор; новая конфигурация требует совместимой резервной копии для старого бинарника.

SSH event, delivery intent, and input position commit in one SQLite transaction. The first run starts at the tail; source selection persists, and a lost position records a visible gap while available records are recovered. Reading cannot be acknowledged before event persistence. Bounded, versioned state extends `meta` without changing existing tables or the main schema version. Reverting to #4 preserves delivery and suspends collection; the old binary needs a compatible configuration backup.
