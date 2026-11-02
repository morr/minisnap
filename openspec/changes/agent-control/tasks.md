# Tasks

## 1. Каркас протокола и структура крейта

- [x] 1.1 Разбить крейт на lib + bin: `src/lib.rs` с публичными модулями, `main.rs` — тонкая обёртка; `cargo build` зелёный.

## 2. Журнал

- [x] 2.1 `src/logbuf.rs`: кольцевой буфер строк (последние 500), макрос-обёртка над `eprintln!`, чтение с offset; перевести существующие `eprintln!`-вызовы модулей на буфер.

## 3. Agent-сервер в приложении

- [x] 3.1 `src/agent_proto.rs`: типы запросов/ответов, идентичность экземпляра, ошибки — всё `serde`.
- [x] 3.2 `src/agent_server.rs`: listener-поток, минимальный HTTP-разбор, `MainJob::Agent` + диспетчер в `tick:`, ответы с идентичностью, таймаут 5 с.
- [x] 3.3 Реестр экземпляров: по файлу на процесс `instances/<pid>.json` рядом с конфигом, запись при старте сервера, снятие при выходе; ключи конфига `agent-server-enabled`/`agent-server-port` и `MINISNAP_AGENT_PORT`.

## 4. Состояние и действия

- [ ] 4.1 Состояние: `AppController::agent_state` + `EditorWindow::agent_state` (документ, выделение, инструмент, цвет/толщина, undo/redo, кадры окон, оверлей, конфиг); `GET /state`.
- [ ] 4.2 Действия уровня приложения: `capture`, `open_latest`, `open_editor`, `close_editor`, `prefs_open/close`, `quit`; ответы «что произошло» (разрешение, оверлей уже активен).
- [ ] 4.3 Оверлей: программное завершение `overlay_select {rect}` через тот же колбэк результата и `overlay_cancel`.
- [ ] 4.4 Операции редактора: `set_tool`, `set_color`, `set_width`, `command`, `undo`, `redo`, `select`, `page` через `OpenShot::apply` + sync; ошибка при закрытом редакторе.

## 5. Скриншоты и журнал

- [x] 5.1 `GET /screenshot`: `composite` через `render_composite` + PNG; `window` через `cacheDisplayInRect` contentView'а; запись в файл, путь и размеры в ответе.
- [x] 5.2 `GET /log` поверх кольцевого буфера.

## 6. MCP-бинарник

- [ ] 6.1 `src/bin/minisnap-mcp.rs`: stdio JSON-RPC (initialize/tools/list/tools/call), HTTP-клиент на TcpStream, выбор инстанса, инструменты `instances`, `state`, `screenshot` (картинка в ответе), `action`, `editor`, `log`; конвенция «не бросать».

## 7. Живая проверка

- [ ] 7.1 Собрать бандл `scripts/build_app.sh release`, запустить, прогнать по HTTP: `/state` пустого приложения, действия, операции редактора на тестовом снимке, скриншоты, журнал; проверить сценарии спеки (порт занят, два инстанса, мёртвый pid).
- [ ] 7.2 Прогнать `minisnap-mcp` stdio-харнессом: initialize, tools/list, каждый инструмент на живом приложении и при незапущенном.

## 8. Документация для агентов

- [ ] 8.1 Скиллы: строка нового модуля в `minisnap-architecture`, механизм сервера в `minisnap-shell`; секция про live-тестирование через MCP вместо скриптов в AGENTS.md.
