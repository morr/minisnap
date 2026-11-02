# Глоссарий

Термины, имена которых несут интерфейсный вес: модули и типы из
архитектурных решений. Нормативное поведение продукта — в `openspec/specs/`;
механика доменов — в скилах `.zcode/skills/`.

## Редактор

- **ToolbarView** — objc2-view тулбара и хрома окна редактора
  (`src/editor_toolbar.rs`). Interface: `set_state(ToolbarState)` на входе,
  `ToolbarEvent` на выходе; AppKit-механика (layout, hover, plates, слайдер
  ширины, поповеры) — внутри.
- **ToolbarState** — снимок состояния тулбара единым значением: выбранный
  инструмент, ширина, цвет, доступность undo/redo, метка файла. Считается
  чистой редукцией `toolbar_state` из состояния документа и покрывается
  юнит-тестами без окна.
- **ToolbarEvent** — все события тулбара одним enum'ом: выбран инструмент,
  изменилась ширина, действие (листание, пресет, тюнер). Новые действия
  (undo, save, …) добавляются вариантами в день появления кнопок.
- **plates** — общая механика подсвеченных подложек кнопок и свотчей палитры
  (`src/plates.rs`). Два потребителя: тулбар и палитра цветов.
- **Машина драга (drag machine)** — чистый модуль взаимодействия канвы:
  события мыши в пикселях → превью, `Command` для журнала и фазы коллаута;
  без AppKit-типов в interface. Держит три семьи состояния — тянущийся
  `Drag`, двухфазный коллаут, рамку кропа/ресайза
  (`src/editor/drag.rs`).
- **arrow-модуль** — параметризованная геометрия Monosnap-стрелки:
  `ArrowParams` с глобальными крутилками (`arrow_params`/`set_arrow_params`),
  силуэт `arrow_outline(_with)` и отрисовка `draw_arrow` в
  `src/editor/arrow.rs` (план: change arrow-render-module). render — чистый
  композит; hit_test берёт геометрию стрелки отсюда.
- **Тюнер стрелки (arrow tuner)** — debug-инструмент под `MINISNAP_ARROW_DEBUG`
  с собственным view-owner'ом: панель крутилок, таблица `ARROW_PARAM_SPECS`,
  промпт (`src/editor_arrow_tuner.rs`, план: change arrow-tuner-module).
  События уходят существующими вариантами `ToolbarEvent::ArrowParam*` — шов
  тулбара не меняется.

## Захват

- **capture::geometry** — единственный конвертер координат (`src/capture.rs`,
  `mod geometry`): переводы между AppKit-пунктами, SCKit-пунктами и пикселями
  изображения идут только его функциями (`display_point_to_pixel`,
  `display_point_to_logical_pixel`, `frame_contains_point`, …); на месте
  вызова допустим лишь оффсет к результату. Инвариант нормативно закреплён
  в спеке `frozen-capture`; карта пространств — в скиле
  `minisnap-architecture`.
- **Презентация цели (target presentation)** — единая процедура показа цели
  оверлея: apply_target, подмена кадра, sampler, veils, fade-in рамки;
  общий путь для `show()` и переключения на другой дисплей
  (`OverlayView::present_target` в `src/overlay.rs`).
- **SurfaceMap** — спаривание поверхностей T0 с экранами (приватный
  `mod presentation` в `src/overlay.rs`); считается один раз за сессию
  оверлея, а не на каждое движение курсора.
- **HUD оверлея** — весь хром над слоем кадра: dim-полосы, лупа, оси, подпись,
  красная рамка с пульсом и FrameSampler за одним interface'ом
  (`struct Hud`: attach/present/update/dim/hide/stop_border_pulse,
  `src/overlay_hud.rs`). Размещение лупы и подписи — чистая функция с
  юнит-тестами; новые HUD-слои не требуют правок present_target.

## Оболочка

- **Машина рекордера хоткеев** — чистые правила записи комбинации в
  настройках: `Recorder::begin(row)` / `feed(mods, keycode) → FeedOutcome`
  (`Recorded`/`Cancelled`/`Swallowed`/`Passthrough`) без AppKit-типов в
  interface (`src/hotkey_recorder.rs`, план: change hotkey-recorder-machine).
  Правила (голый Esc отменяет и глотает, неизвестный keycode глотается,
  пустые модификаторы валидны) покрыты юнит-тестами.
