# Tasks

## 1. Закрытие по ⌘W

- [x] 1.1 В `src/editor_window.rs` расширить ветку матча `keyDown:` канвы `EditorView` с `12 if Command` до `12 | 13 if Command` (⌘Q и ⌘W — закрытие окна) и поправить комментарий ветки («⌘Q closes the viewer» → «⌘Q/⌘W close the viewer»). Верификация: `cargo build` и `cargo clippy` без ошибок и предупреждений в затронутом коде.

## 2. Синхронизация документации и живая проверка

- [x] 2.1 Синхронизировать скилл `minisnap-editor`: в `.zcode/skills/minisnap-editor/references/window-mechanics.md` дополнить карту кейкодов строкой `13+⌘ (⌘W) — закрыть окно` рядом с ⌘Q. Верификация: строка присутствует и не противоречит коду (`src/editor_window.rs`).
- [x] 2.2 Живая проверка: собрать и запустить приложение, открыть редактор (`MINISNAP_DEBUG_OPEN_EDITOR=<снимок>`), нажать ⌘W — окно закрылось, процесс живёт в трее; нажать ⌘Q и Esc — поведение без изменений; во время ввода текста ⌘W не срабатывает. Верификация: наблюдаемое поведение соответствует сценарию «Закрытие по ⌘Q и ⌘W» дельты; после проверки инстанс закрыть (правила «Working with a running app» в AGENTS.md).

## Workflow follow-up

- Заархивировать change после проверки: merge ветки в master и `git worktree remove`, если работа шла в worktree.
