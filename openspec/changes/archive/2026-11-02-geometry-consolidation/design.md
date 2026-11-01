# Design

## Context

Решения пункта 4: дубли координатной арифметики сворачиваются в
`capture::geometry` (src/capture.rs, `pub mod geometry`, сейчас :695–1034);
`display_point_to_pixel` возвращается в работу, через него делается флип
клика в mouseUp; `topmost_window_at` продакшн-вызовов не имеет (перекрыт
списком оконного сервера из `cg_visible_windows`) — удаляется с тестами;
новые и изменённые функции geometry покрываются юнит-тестами по образцу
существующих. Адреса в тексте — по текущему состоянию файлов (capture.rs
1030 строк, overlay.rs 1848; после change'а overlay-target-presentation
номера обзора сместились: инлайн-формулы лупы теперь :1122–1125 и
:1166–1170, флип mouseUp — :1611–1615, containment оверлея — `frame_contains`
в presentation-моде, :229–236). Характер работы — возврат должного, а не
новая логика: каждый дубликат заменяется вызовом уже существующей или
новой чистой функции geometry с теми же семантикой.

## Module

### Новые функции geometry

```rust
/// Display-local AppKit point → logical pixel point of the display image
/// downsampled by `factor` (the sampler's rounded backing scale, never
/// zero). The loupe's sample rect and the coordinate readout both speak
/// logical pixels.
pub fn display_point_to_logical_pixel(
    point: CGPoint,
    display_height_pt: f64,
    scale: f64,
    factor: f64,
) -> CGPoint;  // = display_point_to_pixel(point, h, scale) / factor

/// Whether `point` lies inside `frame` with half-open bounds (origin in,
/// max out) — the "display/frame under the point" test shared by display
/// picking and target switching. Zero-sized frames never contain.
pub fn frame_contains_point(frame: CGRect, point: CGPoint) -> bool;
```

Обе — чистые, без AppKit-вызовов, тестируются как существующие 18.
`display_point_to_logical_pixel` композирует `display_point_to_pixel`
(единственная формула флипа+масштаба); деление на `factor` — переход к
логическим пикселям даунсэмплнутого кадра, factor гарантируется ≥ 1 в
`make_sampler` (:1011, `max(1)`), нулевого деления нет. Семантика
containment сохраняется точь-в-точь: полуоткрытые границы, как в обоих
ручных вариантах; `CGRectContainsPoint` не используется — у него закрытые
границы, и на точке ровно в max-краю он дал бы другой ответ.

### Сворачивание дублей (форма сохранена: interface — поведение то же)

1. **Сэмпл лупы** (overlay.rs:1122–1125) и **подпись координат**
   (:1166–1170): `let logical = geometry::display_point_to_logical_pixel(
   cursor, screen.height, scale, factor);` → `px`/`py` — `logical.x/y` с
   прежними `.round().clamp(half, …)`; подпись — прежний `format!` от
   `logical`. Скругление и кламп остаются на месте вызова: это политика
   отображения лупы, не перевод координат.
2. **Флип клика в mouseUp** (:1611–1615): через `display_point_to_pixel`
   со scale 1.0 —
   `let local = geometry::display_point_to_pixel(up, screen_h, 1.0);`
   затем `point_sckit = CGPoint::new(sckit.origin.x + local.x,
   sckit.origin.y + local.y)`. Масштаб 1 — это ровно «тот же флип, что у
   пиксельного маппинга, в пунктах»: комментарий на месте вызова
   переписывается с этого. Оффсет SCKit-origin дисплея остаётся на месте
   вызова — это прибавление к результату перевода, не сам перевод.
3. **`pick_display`** (capture.rs:105–108): ручной тест заменяется на
   `geometry::frame_contains_point(frame, mouse_sckit)`.
4. **`SurfaceMap::switch_candidate`** (overlay.rs:219–226): локальный
   `frame_contains` (:229–236) удаляется, поиск —
   `geometry::frame_contains_point(p.frame, point)`. Юнит-тесты
   `switch_candidate` (полуоткрытые границы и пр.) остаются без изменений —
   семантика не тронута.

### Удаление `topmost_window_at`

`topmost_window_at` (:830) и его два теста (:1008–1028) удаляются целиком.
Реальных вызовов нет — клик-за-окно разрешается по списку оконного сервера
(`candidate_frames`), а не по этой функции; тест мёртвой функции создавал
ложное ощущение покрытия конвенции. Вместе с ним из geometry уходит импорт
`CGRectContainsPoint` (:697) — единственный его потребитель в модуле; в
`candidate_frames` (:421, внешний модуль) он остаётся: его семантика
(закрытые границы, +zero-size гвард) — это выбор клик-хит-теста, change не
назначен его трогать.

### `#[allow(dead_code)]` на `pub mod geometry`

После возврата `display_point_to_pixel` в работу и добавления двух новых
функций у каждой функции geometry есть продакшн-вызов, поэтому модульный
`#[allow(dead_code)]` (:694) снимается; если билд покажет живой
dead_code-предупреждение — атрибут возвращается точечно (это будет
зафиксировано в tasks.md).

## Seam (промежуточные состояния тасков)

Каждый таск — зелёный build/clippy/test и живой оверлей.

- Таск 1 (containment): `frame_contains_point` появляется в geometry с
  тестами; `pick_display` и `switch_candidate` переходят на неё; локальный
  `frame_contains` удаляется.
- Таск 2 (точка→пиксели + флип): `display_point_to_logical_pixel` с
  тестами; лупа и подпись переходят на неё; флип mouseUp — через
  `display_point_to_pixel`; `topmost_window_at` с тестами удаляется;
  модульный `allow(dead_code)` снимается.
- Таск 3 — domain-skills pass (minisnap-capture, minisnap-architecture).

Порядок выбран по сужению дублирования: containment — одна формула в двух
пространствах, точка→пиксели — две инлайн-формулы плюс возвращение к жизни
существующей функции.

## Verification

- Каждый таск: `cargo build`, `cargo clippy`, `cargo test`; затем live:
  release-билд, экземпляр по PID (stderr в файл), захват через трей-меню,
  сверка лога (`overlay: frozen …`, `click: …`), окна прячутся после каждого
  действия, экземпляр гасится по matched PID (правила «Working with a
  running app»). Лупа/подпись и клик-за-окно — приоритет live-проверки
  таска 2: это единственные пути, чей код реально меняется.
- Если live упирается в активность пользователя или TCC-барьер сессионного
  билда (как в change'ах overlay-target-presentation и более ранних) — не
  ретрайить бесконечно: пометить в tasks.md «ждёт ручной проверки» и
  продолжать.
- Спека: дельта — один ADDED requirement; при архивации синхронизируется в
  `openspec/specs/frozen-capture/spec.md`.
