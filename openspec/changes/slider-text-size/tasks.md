# Tasks

## 1. Команда размера в журнале

- [ ] 1.1 `clamp_block(origin, w, h, frame) -> Point` — чистая функция минимального сдвига блока в кадр, отступ общий с `callout_anchor` (4 px), рядом с ней в `editor/drag.rs` или `editor/mod.rs`; юнит-тесты: no-op когда блок влезает, зажим у каждого края, отступ соблюдён. Verify: `cargo test` — новые тесты зелёные.
- [ ] 1.2 `Command::SetTextSize(id, size)` в `editor/mod.rs`: `apply_cmd` меняет `size` у `Text`/`Callout` и зажимает блок через `clamp_block` от размеров базы; юнит-тесты: размер применился, блок у края сдвинулся, уменьшение после сдвига не отскакивает, команда переживает persist/реплей сессии. Verify: `cargo test` — тесты команды и roundtrip сессии зелёные.
- [ ] 1.3 `EditorDoc::apply_text_size` со слиянием тяг (зеркало `apply_width`) и `OpenShot::apply_text_size` с persist в `editor/session.rs`; юнит-тест: тяга из нескольких шагов — один undo. Verify: `cargo test` — тест слияния зелёный.

## 2. Машина драга

- [ ] 2.1 `DragMachine::set_pending_callout_size(size, image) -> Option<Point>` в `editor/drag.rs`: обновляет `PendingCallout.size`, возвращает переанкеренную позицию блока; юнит-тест: рост размера переанкерит блок у основания, `width`/`tip` не трогаются, без pending — `None`. Verify: `cargo test` — тест машины зелёный.

## 3. Тулбар: двуличный слайдер

- [ ] 3.1 В `editor_toolbar.rs`: константы `TEXT_SIZE_MIN`/`TEXT_SIZE_MAX`; `ToolbarState.slider_value` + `ToolbarState.slider_range` вместо `stroke_width`; параметр `text_input: Option<f64>` в редукции `toolbar_state`; обновить юнит-тесты редукции: текстовый режим даёт размер и текстовый диапазон, толщинный режим прежний, зажим выделения вне текстового режима. Verify: `cargo test editor_toolbar` — тесты редукции зелёные.
- [ ] 3.2 `WidthSliderView`: диапазон в ivars, `drag_to`/`knob_x`/`setDoubleValue:` считают через активный диапазон; `ToolbarView::set_state` протаскивает диапазон; `ToolbarEvent::WidthChanged` → `SliderChanged` вместе с местом emit. Verify: `cargo build` и `cargo test` зелёные.

## 4. Окно: текстовая ветка слайдера

- [ ] 4.1 В `editor_window.rs`: ivar `text_size`; `begin_text_edit` выставляет его (элемент для `Existing`, дефолт для `New`); коммит `New` обычного текста — `clamp_block` на origin и размер из ivar в элемент. Verify: `cargo build` зелёный, юнит-тесты не красные.
- [ ] 4.2 Обработчик `SliderChanged`: текстовая ветка по `text_target` (коллаут `New` — `set_pending_callout_size` + рамка; обычный `New` — шрифт + рамка с зажимом; `Existing` — `apply_text_size` + шрифт + рамка), толщинная ветка прежняя; `image_size` читается до `shot.borrow_mut()`; `stroke_width` в текстовой ветке не трогается. Verify: `cargo build` зелёный; ручной прогон веток в 5.1.
- [ ] 4.3 `refit_callout_editor`: ветка `Existing` зажимает блок `clamp_block` при невлезании. Verify: `cargo build` зелёный; сценарий правки в 5.1.

## 5. Живая проверка и скилл

- [ ] 5.1 Живой прогон трёх сценариев спеки на запущенном приложении (правила инстансов из AGENTS.md): новый текст у края кадра + тяга слайдера — кегль растёт, блок смещается в кадр; коллаут во второй фазе + слайдер — текст растёт сбоку от стрелки, указатель прежней толщины; правка существующего текста + слайдер — размер меняется живо, файл обновляется сразу, вся тяга = один undo. Verify: наблюдаемое поведение совпадает с добавленными сценариями спеки.
- [ ] 5.2 Скилл `minisnap-editor`: в `references/window-mechanics.md` — текстовый режим слайдера и порядок займов в его обработчике; в `references/doc-and-sessions.md` — `SetTextSize` в списке команд; ровно то, что изменилось. Verify: текст скилла соответствует коду.

## Workflow follow-up

- После ревью — `/opsx:archive`: слить ветку в master (fast-forward), удалить worktree при наличии, финальный коммит архивации.
