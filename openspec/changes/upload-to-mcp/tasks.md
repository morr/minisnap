# Tasks

## 1. Протокол и состояние

- [x] 1.1 `src/agent_proto.rs`: вариант `AgentAction::Upload` (форма `{"Upload":{}}`, doc-comment — семантика кнопки Upload, async-ответ); юнит-тест: сериализация/разбор `"Upload"` туда-обратно и в форме с пустым объектом; временное плечо-заглушка в `agent_action`, чтобы match оставался исчерпывающим и сборка была зелёной (заменяется в 2.1). Проверка: `cargo test`.
- [ ] 1.2 `src/agent_proto.rs` + `src/editor_window.rs`: поле `uploading: bool` в `EditorState` (заполняется из Cell'а view в `agent_state`); геттеры `EditorWindow::shot_path()` и `EditorWindow::is_uploading()` — тонкие делегаты к view. Проверка: `cargo test`.

## 2. Действие в приложении

- [ ] 2.1 `src/app.rs`: из `handle_upload_request` выделен общий хвост `start_upload` (состояние кнопки → `upload_file` → откат при локальной ошибке); маршрут `AgentAction::Upload` в `agent_action` через новый `agent_upload` — rejected (нет редактора/снимка, в полёте, не настроено — без диалога) или `initiated`; строка успеха в `upload_finished` несёт публичный URL. Проверка: `cargo test`.

## 3. Мост и описание для агентов

- [ ] 3.1 `src/bin/minisnap-mcp.rs`: описание `action` перечисляет `{"Upload":{}}`, описание `state` упоминает `uploading`; тест `tools_list` проверяет упоминание Upload в описании `action`. `AGENTS.md`: раздел про MCP — форма в списке `action`, `uploading` в описании `state`, семантика `initiated` (поллить `editor.uploading`). Проверка: `cargo test`.

## 4. Скиллы и живая проверка

- [ ] 4.1 Доменные скиллы: `minisnap-shell` (список действий `/action`, семантика `initiated`), `minisnap-output` (входы запуска загрузки: кнопка, хоткей, агентское действие) — обновить по диффу change'а. Проверка: сверка с кодом.
- [ ] 4.2 Живая проверка worktree-инстанса на `MINISNAP_AGENT_PORT=15791`: `OpenLatest` → `Upload` → `initiated` → поллинг `uploading` → URL в `/log` и в буфере (`pbpaste`), GET по URL отвечает; `rejected` при `enabled=false` в конфиге (правка при остановленном инстансе, точное восстановление).
