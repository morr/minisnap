# Proposal

## Why

Конвенция «перевод координат — только через `capture::geometry`» держится на
честном слове и уже протекла в обе стороны (обзор домена, кандидат 2):

- `geometry::display_point_to_pixel` (src/capture.rs:823) и
  `geometry::topmost_window_at` (:830) не имеют ни одного продакшн-вызова —
  только тесты. Оверлей вместо первого пишет ту же формулу инлайн, дважды:
  `(cursor.x * scale / factor)` и `((screen.height - cursor.y) * scale / factor)`
  в сэмпле лупы (src/overlay.rs:1122–1125) и в подписи координат
  (:1166–1170). Вторую вытеснил `candidate_frames` в capture.rs.
- Перевод клика display-local AppKit → SCKit-глобальные сделан вручную в
  mouseUp (src/overlay.rs:1611–1615), и комментарий рядом признаётся: «the
  click converts from display-local AppKit the same way the pixel mapping
  does» — автор на месте вызова заново вывел конвертацию, которой в geometry
  нет.
- Формула «точка в кадре» (полуоткрытые границы) написана руками дважды:
  `pick_display` (src/capture.rs:105–108, SCKit-пространство) и
  `frame_contains` в presentation-моде оверлея (src/overlay.rs:229–236,
  AppKit-пространство).

Это тот случай, когда чистые функции вынесены ради тестируемости, а реальный
баг живёт на месте вызова: клик-точка, приходящая в покрытый тестами
`candidate_frames`, посчитана ручной конвертацией в mouseUp — ошибись там
флип Y, ни один существующий тест не заметил бы.

## What Changes

- В `capture::geometry` появляются две функции: `display_point_to_logical_pixel`
  (display-local AppKit → логический пиксель даунсэмплнутого кадра, через
  `display_point_to_pixel`) и `frame_contains_point` (полуоткрытый
  containment «точка в кадре»). Юнит-тесты — по образцу существующих.
- Дубли на местах вызова сворачиваются в вызовы geometry: сэмпл лупы и
  подпись координат — через `display_point_to_logical_pixel`; флип клика в
  mouseUp — через `display_point_to_pixel` (масштаб 1, сверху оффсет SCKit
  origin дисплея); `pick_display` и `SurfaceMap::switch_candidate` — через
  `frame_contains_point`. Локальный `frame_contains` оверлея удаляется.
- `display_point_to_pixel` возвращается в работу (два вызова). Мёртвый
  `topmost_window_at` продакшн-вызовов не получает (перекрыт списком
  оконного сервера) — удаляется вместе со своими тестами.
- Наблюдаемое поведение режима не меняется: показ, лупа, оси, подпись,
  протяжка, клик-за-окно, Esc — всё как раньше.
- Спека `frozen-capture` получает инвариант: перевод между тремя системами
  координат (AppKit-пункты, SCKit-пункты, пиксели изображения) — только
  через `capture::geometry`.

## Capabilities

### New Capabilities

(нет)

### Modified Capabilities

- `frozen-capture` — ADDED requirement: единственный конвертер координат.

## Impact

- Код: `src/capture.rs` (geometry: +2 функции, +тесты, −`topmost_window_at`
  с его тестами; `pick_display` — containment через geometry) и
  `src/overlay.rs` (сэмпл лупы, подпись координат, флип mouseUp,
  `switch_candidate` — через geometry; локальный `frame_contains` удаляется).
- Скиллы: `.zcode/skills/minisnap-capture/SKILL.md` и
  `.zcode/skills/minisnap-architecture/SKILL.md` обновляются в том же
  change'е (строка geometry в карте модулей, инвариант containment).
- GLOSSARY.md: после архивации — снять машинерию «план»-терминов (все
  change'ы обзора доехали), занести `capture::geometry`.
- Риск низкий: поведение не меняется, все переводы уже покрыты или
  покрываются юнит-тестами geometry; live-проверка оверлея — по итогам
  тасков.
