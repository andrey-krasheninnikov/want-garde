# Issue tracker: GitHub

Репозиторий / Repository: `andrey-krasheninnikov/want-garde`.

Спецификация и каждый тикет публикуются отдельными GitHub Issues.
Каждый текст содержит русскую и английскую версии.
The spec and each ticket are separate GitHub issues.
Each body contains Russian and English versions.

Use the `gh` CLI. Pass multiline bodies through `--body-file`.
Read existing issues before creating new ones.
After a write, read back the resulting content and relationships.
Reconcile ambiguous results before retrying.

Тикеты ссылаются на спецификацию через раздел Parent.
Зависимости задаются native issue dependencies и перечисляются в Blocked by.
Если native dependencies недоступны, Blocked by остаётся источником зависимостей.
Не изменять и не закрывать родительскую спецификацию при публикации тикетов.

Tickets reference the spec in Parent.
Use native issue dependencies and list blockers in Blocked by.
If native dependencies are unavailable, use Blocked by.
Do not edit or close the parent spec while publishing tickets.

Use the label mapping in `docs/agents/triage-labels.md`.

PRs as a request surface: no.
