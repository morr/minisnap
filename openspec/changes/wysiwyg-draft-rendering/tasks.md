# Tasks

## 1. Машина драга: черновик текста и коллаута

- [x] 1.1 Ввести в `DragMachine` (`src/editor/drag.rs`) состояние набора текста `TextDraft` (цель: origin+color нового / id правки; кегль; живая строка) с API `begin_text_draft` / `update_text` / `update_text_size` / `commit_text` / `cancel_text`; живая строка у `PendingCallout`. Единый билдер `text_draft_kind`: `text::measure_block` → `clamp_block` / `callout_anchor`, черновик отдаётся через `preview()` с настоящим текстом и якорем. Проверка: `cargo test -p minisnap editor::drag`.
- [x] 1.2 Юнит-тесты инварианта «черновик == коммит» в `src/editor/drag.rs`: для нового текста и коллаута на сетке строк (пустая → символ → длинная → многострочная), кеглей и положений у краёв кадра — `preview().kind` совпадает с kind из `commit_text` до и после каждого keystroke; плюс тесты клампа растущего блока у края (блок уезжает внутрь по ходу роста, не прыгает). Проверка: `cargo test -p minisnap editor::drag`.
- [x] 1.3 `commit_text` закрывает и правку существующего: пустой ввод — `DeleteElement`, изменившийся — `EditText`, нетронутый — `None`; пустой коллаут — сброс `PendingCallout` без команды. Существующие тесты `commit_callout_text`/`text_tool_starts_new_text` перевести на новый API. Проверка: `cargo test -p minisnap editor::`.

## 2. Окно: невидимый прокси ввода

- [x] 2.1 `src/editor_window.rs`: поле становится прокси — прозрачный цвет текста, видимая каретка (окраска field editor'а `insertionPointColor`; если не красится независимо — каретка канвы по замеру префикса до insertion point), рамка поля = прямоугольник черновика. `controlTextDidChange` → `machine.update_text` + redraw; коммит и слайдер кегля идут через машину (`update_text_size`). Проверка: `cargo test -p minisnap` + ручная сборка `scripts/build_app.sh debug`.
- [x] 2.2 Разобрать геометрические контракты видимого поля: удалить `TEXT_EDITOR_WIDTH_PT` и порог `.max(9.0)`, `text_editor_size`/`text_editor_frame` сводятся к рамке черновика (набивка и `0.19·font_pt` — только хром каретки), `commit_text_editor` и `refit_callout_editor` теряют сборку геометрии (остаётся жизненный цикл поля). Проверка: `cargo test -p minisnap` без новых предупреждений.

## 3. Правка существующего: якорь по зажиму

- [ ] 3.1 `apply_cmd::EditText` в `src/editor/mod.rs` пересчитывает якорь текстового и коллаут-элемента через `clamp_block` для нового текста (прецедент — `SetTextSize`/`clamp_text_block`); тест: реплей журнала с `EditText` у края кадра воспроизводит тот же якорь, undo возвращает исходный. Проверка: `cargo test -p minisnap editor::tests`.

## 4. Перенос и тяга конца: глубина и маркеры

- [ ] 4.1 `preview()` в `src/editor/drag.rs` возвращает полный список отрисовки: призрак замещает элемент на его индексе, черновики — в конец; `EditorView::draw` рисует один список. Тест: при переносе элемента с перекрытием порядок отрисовки сохраняется (индекс призрака == индекс элемента). Проверка: `cargo test -p minisnap editor::`.
- [ ] 4.2 `draw_markers` (`src/editor_window.rs`) берёт геометрию выделенного элемента из списка машины, а не из документа — маркеры следуют за призраком при переносе/тяги конца. Проверка: `cargo test -p minisnap` + живая тяга выделенного элемента (MCP `screenshot`).

## 5. Живая проверка

- [ ] 5.1 Пересобрать bundle (`scripts/build_app.sh release`), перезапустить экземпляр по PID, прогнать через minisnap MCP: ввод текста у правого/нижнего края (черновик с ореолом, каретка у глифов, Enter без смещения), коллаут (хвост у блока при вводе), правка существующего, перенос с перекрытием (глубина, маркеры), window-скриншот канвы. Окна спрятать после проверки, тестовые снимки — во временный `save_dir`. Проверка: протокол прогона в журнале change'а.

## 6. Документация домена

- [ ] 6.1 Обновить доменный скилл `minisnap-editor` (`.zcode/skills/minisnap-editor/SKILL.md` и его `references/window-mechanics.md`): механика ввода текста (прокси-поле + черновик в машине), `preview()` как полный список отрисовки, зажим `EditText`. Проверка: сверка утверждений скилла с фактическим кодом после задач 1–4.

## Workflow follow-up

- Архивация change'а (`/opsx:archive`): сливает дельту в `openspec/specs/shot-editor/spec.md`, коммит, работа велась в мастер-чекауте — merge/worktree-шаги не нужны.
