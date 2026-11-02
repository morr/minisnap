# Proposal

## Why

`src/editor/render.rs` — единственный модуль домена редактора с двумя
обязанностями равного веса: композит (что получает файл и буфер обмена) и
геометрия стрелки (как выглядит главный элемент продукта). Семья стрелки
занимает ~390 строк (~L207–602) — почти половину файла — и является
единственным местом домена с глобальным мутабельным состоянием
(`ARROW_PARAMS`, RwLock-глобал). Правки стрелки идут вперемешку с
композитом, а тестируемость силуэта привязана к чужому модулю.

## What Changes

- Новый модуль `src/editor/arrow.rs`: `ArrowParams` (+`DEFAULT`),
  `arrow_params`/`set_arrow_params` с глобалом `ARROW_PARAMS`,
  `arrow_outline(_with)`, `draw_arrow` и их приватные помощники
  (`ArrowFrame`, `quad`, `round_corners`, `pt_dist`, `arrow_gradient`).
  Interface уже сложился (тип + 4 функции) — нового не изобретается.
- `render.rs` остаётся чистым композитом: `draw_element` зовёт
  `arrow::draw_arrow`; re-export `pub(crate) use` в `render` сохраняет пути
  вызывающих (`editor_toolbar.rs` — `ARROW_PARAM_SPECS` и иконка коллаута,
  `editor_window.rs` — обработчики `ToolbarEvent::ArrowParam*`).
- Зависимость `hit_test → render` за `arrow_outline` мигрирует в
  `hit_test → arrow` (уточнение: связь `hit_test → render` сохраняется за
  `callout_tail`, который остаётся в render — см. design).
- Тесты силуэта (`arrow_matches_the_monosnap_shape`,
  `arrow_outer_edge_bows_out`, `arrow_params_reshape_the_outline`)
  переезжают в `arrow.rs` целиком и гоняются против `arrow_outline_with`
  как раньше.
- На месте в `render.rs` остаются: `callout_tail`, `pixelate_region`,
  `bitmap_context`, композит и их тесты.
- Наблюдаемое поведение не меняется.

## Capabilities

### New Capabilities

(нет)

### Modified Capabilities

(нет — чистый рефакторинг: наблюдаемое поведение стрелки и композита не
меняется, спека `shot-editor` не меняется; change помечен `skip_specs: true`)

## Impact

- Код: `src/editor/render.rs` (−~390 строк: семья стрелки и её тесты
  переезжают), новый `src/editor/arrow.rs` (~430 строк с тестами),
  `src/editor/mod.rs` (+1 `pub(crate) mod`), `src/editor/hit_test.rs`
  (2 пути импорта).
- Скиллы: `.zcode/skills/minisnap-editor/SKILL.md` (таблица модулей:
  строка `src/editor/arrow.rs`, зависимость `hit_test → arrow`) —
  обновляется в том же change'е.
- GLOSSARY.md: термин «arrow-модуль» уже внесён с пометкой «план: change
  arrow-render-module» — пометка снимается при архивации.
- Риск низкий: код переезжает построчно, вызовы и видимость сохраняются;
  live-проверка стрелки в редакторе — по итогам тасков, при TCC-барьере
  сессионного билда ждёт ручной проверки.
