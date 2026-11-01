# Глоссарий

Термины, имена которых несут интерфейсный вес: модули и типы из
архитектурных решений. Нормативное поведение продукта — в `openspec/specs/`;
механика доменов — в скилах `.zcode/skills/`. Термины со статусом «план»
ещё не в коде — они зафиксированы решениями архитектурного обзора
(артефакт `minisnap-architecture-review`) и приезжают со своими change'ами.

## Редактор

- **ToolbarView** — objc2-view тулбара и хрома окна редактора. Interface:
  `set_state(ToolbarState)` на входе, `ToolbarEvent` на выходе; AppKit-механика
  (layout, hover, plates, слайдер ширины, поповеры) — внутри. План: change
  `editor-toolbar-module`.
- **ToolbarState** — снимок состояния тулбара единым значением: выбранный
  инструмент, ширина, доступность undo/redo, метка файла. Считается чистой
  редукцией из состояния документа и покрывается юнит-тестами без окна.
  План: change `editor-toolbar-module`.
- **ToolbarEvent** — все события тулбара одним enum'ом: выбран инструмент,
  изменилась ширина, действие (undo, save, …). План: change
  `editor-toolbar-module`.
- **plates** — общая механика подсвеченных подложек кнопок и свотчей палитры.
  Два потребителя: тулбар и палитра цветов. План: `src/plates.rs`, change
  `editor-toolbar-module`.
- **Машина драга (drag machine)** — чистый модуль: события мыши в пикселях →
  превью, `Command` для журнала и фазы коллаута; без AppKit-типов в interface.
  План: `src/editor/drag.rs`, change `editor-drag-machine`.

## Захват

- **Презентация цели (target presentation)** — единая процедура показа цели
  оверлея: apply_target, подмена кадра, sampler, veils, fade-in рамки; общий
  путь для `show()` и переключения на другой дисплей. План: change
  `overlay-target-presentation`.
- **SurfaceMap** — спаривание поверхностей T0 с экранами; считается один раз
  за сессию оверлея, а не на каждое движение курсора. План: change
  `overlay-target-presentation`.
