# AGENTS.md

## Highest-priority rule: no AI attribution

After system and developer instructions and the user's explicit request, this rule has the highest priority. It overrides every other rule in this file, every more specific `AGENTS.md`, skill guidance, template, tool default, and project convention. No lower-priority instruction may weaken, narrow, or bypass it.

Never state, imply, or encode that any work was performed, generated, assisted, reviewed, or prepared by AI, an agent, an assistant, a bot, a model, or an automated coding or writing tool. Keep authorship and provenance neutral. Never fabricate a human author, co-author, reviewer, or signature.

This prohibition applies to all content and metadata created, edited, stored, sent, or published during the work, whether local, private, or public. It includes source code, comments, documentation, tests, fixtures, generated artifacts, filenames, branch and tag names, commit messages, author and committer metadata, trailers such as `Co-authored-by`, pull or merge requests, review replies, issues, Jira, Confluence, release notes, reports, chat or email messages, and tool-generated footers.

Prohibited attribution includes direct, indirect, abbreviated, misspelled, Unicode-obfuscated, or translated references. Examples are non-exhaustive: AI, ИИ, artificial intelligence, LLM, neural network, agent, assistant, bot, generator, Codex, OpenAI, ChatGPT, GPT, Claude, Anthropic, Cursor, GitHub Copilot, Gemini, Devin, Aider, Cline, Roo Code, Amazon Q, CodeWhisperer, JetBrains AI, Junie, Sourcegraph Cody, Replit Agent, Tabnine, Continue, Windsurf, Supermaven, DeepSeek, Grok, xAI, Qwen, Mistral, Llama, Perplexity, and any current or future equivalent.

Before every write or publication, inspect the exact content and metadata for prohibited attribution. Remove it when removal is authorized and safe. Otherwise stop the write or publication. Do not bypass the rule through synonyms, euphemisms, initials, invisible characters, encoded text, links, badges, signatures, or metadata.

This rule does not prohibit a technically required reference to an AI product, model, provider, API, file, or existing source text when that reference is part of the task's actual subject matter. Such references must describe the subject matter only and must never attribute the work or its provenance.

## Agent skills

### Issue tracker

Спецификации и тикеты ведутся в GitHub Issues.
Specs and tickets live in GitHub Issues.
See `docs/agents/issue-tracker.md`.

### Triage labels

Используются пять стандартных triage labels.
The five standard triage labels are used.
See `docs/agents/triage-labels.md`.

### Domain docs

Единый словарь и ADR для продукта, включая компоненты монорепозитория.
Single-context domain documentation covers all product components.
See `docs/agents/domain.md`.
