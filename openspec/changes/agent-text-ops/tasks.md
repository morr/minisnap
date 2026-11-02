# Tasks

## 1. Протокол

- [x] 1.1 `src/agent_proto.rs`: `AgentEditorOp::BeginText { target, text }` / `FinishText { commit }`, enum `AgentTextTarget` (New/NewCallout/Existing), поля `EditorState.text_editor_frame`/`text_editor_text`.

## 2. Реализация

- [x] 2.1 `src/editor_window.rs`: `agent_begin_text` (guard'ы: документ/открытый ввод/кадр/занятость/вырожденность; коллаут через машину драга down→drag_to→up; вставка строки через stringValue + refit), `agent_finish_text` (commit → `commit_text_editor`, cancel → `discard_text_editor`), `text_editor_frame_px` для состояния.
- [x] 2.2 `src/app.rs`: плечи `BeginText`/`FinishText` в `agent_editor`.

## 3. Мост и проверка

- [x] 3.1 `src/bin/minisnap-mcp.rs`: описание тулов `editor` с новыми операциями; сборка, юнит-тесты, живая проверка по HTTP (открытие/вставка/коммит/отмена, коллаут, guard'ы).
