# Proposal

## Why

Debug-тюнер стрелки — панель слайдеров под `MINISNAP_ARROW_DEBUG`, инструмент
разработки, не продукт, — занимает ~370 строк продуктового файла
`src/editor_toolbar.rs` (~L1547–1848: спеки `ARROW_PARAM_SPECS`, промпт
`arrow_params_prompt`, панель `arrow_tuner_panel`), плюс четыре objc2-селектора
(~L705–768) и ivars-поле `arrow_param_rows` в классе тулбара. Дебаг-нагрузка
сварена с продуктовым швом: `ARROW_PARAM_SPECS` — единственная причина, по
которой тулбар знает типы рендера стрелки (`render::ArrowParams`).

## What Changes

- Новый модуль `src/editor_arrow_tuner.rs` (рядом с `editor_toolbar.rs`):
  спеки (`ArrowParamSpec`, `ARROW_PARAM_SPECS`), промпт
  (`arrow_params_prompt`) и панель (`arrow_tuner_panel`) с собственным
  маленьким objc2-view-owner'ом — наследником `NSBox`, который сам держит
  rows (слайдер + поле каждой строки), target/action замыкаются на него,
  наружу — готовые `ToolbarEvent::ArrowParam*` через on-event-колбэк того
  же вида, что у тулбара.
- `editor_toolbar.rs` худеет ~1987 → ~1620 строк: уходят панель, спеки,
  промпт, четыре селектора и ivars-поле `arrow_param_rows`. Варианты
  `ArrowParam*` остаются в enum'е `ToolbarEvent` — внешний шов
  `ToolbarView` (`set_state(ToolbarState)`/`ToolbarEvent`) не меняется.
- `editor_window.rs`: хук `MINISNAP_ARROW_DEBUG` строит панель из нового
  модуля с тем же замыканием на `handle_toolbar_event`; обработчики
  `ArrowParam*` берут `ARROW_PARAM_SPECS` из модуля тюнера и применяют их
  как раньше.
- Тюнер берёт `ArrowParams` из `src/editor/arrow.rs` напрямую (пункт 2
  цепочки применён).
- Оставшиеся потребители семьи стрелки вне `editor/` — иконка коллаута в
  тулбаре (`render::draw_arrow`) и три обращения обработчиков окна —
  переключаются с путей `render::` на `arrow::` напрямую; re-export семьи
  в `render.rs` схлопывается (запланировано design'ом `arrow-render-module`).
- Тест промпта тюнера (`arrow_prompt_carries_every_param_and_the_steps`)
  переезжает в новый модуль вместе с промптом; тесты редукции остаются в
  `editor_toolbar.rs` — сама редукция (`toolbar_state`) не переезжает.
- Наблюдаемое поведение не меняется (debug-инструмент под
  `MINISNAP_ARROW_DEBUG`).

## Capabilities

### New Capabilities

(нет)

### Modified Capabilities

(нет — чистый рефакторинг: наблюдаемое поведение не меняется, тюнер не
продукт и в спеке `shot-editor` не описан; change помечен `skip_specs: true`)

## Impact

- Код: новый `src/editor_arrow_tuner.rs` (~330 строк с тестами),
  `src/editor_toolbar.rs` (−~370 строк: семья тюнера и её тест переезжают,
  четыре селектора и ivars-поле уходят), `src/editor_window.rs` (импорты и
  хук), `src/editor/render.rs` (−4 строки: re-export семьи схлопывается),
  `src/main.rs` (+1 `mod`).
- Скиллы: `.zcode/skills/minisnap-editor/SKILL.md` (таблица модулей: строка
  `src/editor_arrow_tuner.rs`, строка тулбара без тюнера; заметка: варианты
  `ArrowParam*` — собственность тюнера) и строки тулбара в
  `.zcode/skills/minisnap-architecture/SKILL.md` — обновляются в том же
  change'е.
- GLOSSARY.md: термин «Тюнер стрелки (arrow tuner)» внесён с пометкой
  «план: change arrow-tuner-module» — пометка снимается при архивации.
- Риск низкий: код переезжает построчно, шов и поведение не меняются;
  live-проверка тюнера под `MINISNAP_ARROW_DEBUG` — по итогам тасков, при
  TCC-барьере сессионного билда ждёт ручной проверки.
