# Tasks

## 1. Тулбар: снимок и краска

- [x] 1.1 В `editor_toolbar.rs`: константа `PAGE_DISABLED_TINT` (`#999999`) рядом с `ICON_TINT`; поля `can_page_prev: bool`, `can_page_next: bool` в `ToolbarState`; параметры `can_page_prev`/`can_page_next` в редукции `toolbar_state` с пробросом в снимок; обновить существующие юнит-тесты редукции под новую сигнатуру, добавить тест проброса (край, середина, оба false). Verify: `cargo test editor_toolbar` — тесты редукции зелёные.
- [x] 1.2 В `editor_toolbar.rs`: проход `update_page_buttons(can_page_prev, can_page_next)` — тинг шевронов по доступности + ivars для hover/клика; вызов из `set_state`; skip листающих тегов при покраске тинга в `update_tool_highlight`; guard в `__page_clicked` (disabled — события нет); hover-скип disabled-кнопки в `hover_at`. Verify: `cargo build` и `cargo test` зелёные.

## 2. Окно: доступность листания

- [x] 2.1 В `editor_window.rs` `toolbar_state()`: `output::sorted_shots(&ivars.save_dir.borrow())`, позиция открытого снимка; `can_page_prev = index > 0`, `can_page_next = index + 1 < len`; снимка нет или не найден в листинге — оба `false`; передать в редукцию. Verify: `cargo build` зелёный, юнит-тесты не красные.

## 3. Живая проверка и скилл

- [ ] 3.1 Живой прогон сценариев спеки на запущенном приложении (`scripts/build_app.sh release`, правила инстансов из AGENTS.md): открыть редактор со свежим снимком папки с несколькими снимками — «следующий» disabled `#999999`, «предыдущий» активен; листание в самый старый — «предыдущий» disabled; hover на disabled — без подсветки, клик ничего не меняет; после листания disabled-шеврон остаётся `#999999`; папка из одного снимка — обе disabled. Verify: наблюдаемое поведение совпадает с добавленными сценариями спеки.
- [ ] 3.2 Скилл `minisnap-editor`: в `references/window-mechanics.md` — состав снимка `ToolbarState` (доступность листания) и собственная краска/guard листающей пары со скипами в проходах подсветки и ховера; ровно то, что изменилось. Verify: текст скилла соответствует коду.

## Workflow follow-up

- После ревью — `/opsx:archive`: слить ветку в master (fast-forward), удалить worktree при наличии, финальный коммит архивации.
