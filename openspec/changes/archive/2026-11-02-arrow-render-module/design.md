# Design

## Context

`src/editor/render.rs` (~870 строк на master@9ab7490) совмещает композит
(`render_composite`, `draw_elements`, `pixelate_region`, `bitmap_context`)
и всю геометрию стрелки (~390 строк, ~L207–602): параметризация
(`ArrowParams` + RwLock-глобал `ARROW_PARAMS`), силуэт
(`arrow_outline(_with)` над `ArrowFrame`/`quad`/`round_corners`), заливка
(`arrow_gradient`) и отрисовка (`draw_arrow`), плюс три теста силуэта.
Потребители семьи: `render::draw_element` (стрелка и хвост коллаута),
`hit_test.rs` (силуэт для попаданий), `editor_toolbar.rs` (спеки тюнера
`ARROW_PARAM_SPECS` типизированы `render::ArrowParams`; иконка коллаута
рисует `render::draw_arrow`; промпт тюнера читает `render::arrow_params`),
`editor_window.rs` (обработчики `ToolbarEvent::ArrowParam*` пишут
`render::set_arrow_params`).

## Goals / Non-Goals

**Goals:**

- Геометрия стрелки живёт в одном модуле `src/editor/arrow.rs` за
  сложившимся interface'ом (тип + 4 функции); единственное глобальное
  мутабельное состояние домена концентрируется в нём.
- `render.rs` — чистый композит: ни одной строки про форму стрелки.
- Тесты силуэта переезжают вместе с кодом и продолжают гоняться против
  `arrow_outline_with`.
- Наблюдаемое поведение не меняется: построчный переезд, те же видимости,
  те же пути для всех вызовов вне `editor/`.

**Non-Goals:**

- Не менять interface стрелки (сигнатуры, имена, видимость) — он сложился.
- Не трогать тюнер: `editor_toolbar.rs` продолжает видеть семью через
  пути `render::` (re-export); вынос тюнера — следующий change цепочки
  (`arrow-tuner-module`), который переключит его на `arrow` напрямую.
- Не менять швы прошлого раунда: `ToolbarView`
  (`set_state(ToolbarState)`/`ToolbarEvent`), машина драга,
  `present_target`/`SurfaceMap`, `capture::geometry`, модуль `overlay_hud`.
- Не менять нормативное поведение (спека `shot-editor`).

## Decisions

1. **Файл модуля — `src/editor/arrow.rs`**, в `editor/mod.rs` —
   `pub(crate) mod arrow;` (по образцу `drag`; наружу поддерева `editor/`
   семью не отдаём — потребители вне `editor/` идут через re-export в
   `render`). Альтернатива `src/editor/render/arrow.rs` (каталог-модуль)
   отклонена: переименование `render.rs` ради одного переезда трогало бы
   все импорты без выигрыша.

2. **Переезд построчный, без переписывания.** В `arrow.rs` уходят:
   `ArrowParams` (+`DEFAULT`), глобал `ARROW_PARAMS` с
   `arrow_params`/`set_arrow_params`, `ArrowFrame`, `arrow_outline`,
   `arrow_outline_with`, `quad`, `round_corners`, `pt_dist`,
   `arrow_gradient`, `draw_arrow` — тела без изменений, меняются только
   ссылки `super::Point`/`super::Rgba` на крейт-импорты
   `use super::{Point, Rgba};` (эквивалентно: те же типы из `editor/mod.rs`).
   Из импортов CoreGraphics модулю нужны `CGPoint`, `CGColor`,
   `CGColorSpace`, `CGGradient`, `CGGradientDrawingOptions`, `CGContext`.

3. **Re-export в `render` сохраняет пути вызывающих.**
   `pub(crate) use super::arrow::{arrow_params, draw_arrow, set_arrow_params, ArrowParams};`
   в `render.rs` — ровно те имена, на которые ссылаются
   `editor_toolbar.rs` (`ArrowParams` в спеках и промпте, `arrow_params`,
   `draw_arrow` для иконки коллаута) и `editor_window.rs`
   (`arrow_params`, `set_arrow_params`, `ArrowParams::DEFAULT`).
   `arrow_outline(_with)` через `render` больше не реэкспортируются — их
   единственный внешний потребитель (`hit_test`) мигрирует (п. 4), и
   держать неиспользуемый re-export значило бы сохранять ложный след.
   `draw_element` в `render.rs` зовёт `arrow::draw_arrow` напрямую.

4. **Зависимость `hit_test → arrow`.** Оба места
   (`ElementKind::Arrow` и хвост коллаута) меняют
   `super::render::arrow_outline` на `super::arrow::arrow_outline`.
   Уточнение к решениям кандидата («сегодня только за arrow_outline»):
   связь `hit_test → render` не исчезает полностью — `callout_tail`
   (L78 hit_test) остаётся в `render.rs` по решениям (это композитная
   склейка текстового блока и хвоста, не геометрия стрелки), поэтому
   точная формулировка: за геометрию стрелки hit_test больше render не
   знает, за `callout_tail` — знает по-прежнему.

5. **Тесты силуэта переезжают целиком.** `arrow_matches_the_monosnap_shape`,
   `arrow_outer_edge_bows_out`, `arrow_params_reshape_the_outline` — в
   `#[cfg(test)] mod tests` в `arrow.rs`. Первые два помощника
   (`solid_frame`, `pixel_bytes` из `editor::mod::tests`, а также
   `render_composite` для пиксельного теста) берутся по путям
   `super::super::tests::*` и `super::super::render::render_composite` —
   это тестовая зависимость `arrow → render`, цикла в продакшн-коде нет
   (продакшн-стрелка не знает про render). `arrow_outline`,
   `arrow_outline_with`, `ArrowFrame`, `pt_dist` видны тестам через
   `super::*` внутри модуля. Тесты композита
   (`composite_renders_at_base_size` — с элементом-стрелкой как
   интеграционной проверкой пути `draw_element → arrow::draw_arrow`,
   `pixelate_*`, `callout_tail_*`) остаются в `render.rs`.

6. **Видимости не меняются.** Все переезжающие функции и `ArrowParams`
   остаются `pub(crate)`; приватные помощники (`ArrowFrame`, `quad`,
   `round_corners`, `pt_dist`, `arrow_gradient`) — приватные в модуле.

## Risks / Trade-offs

- [Re-export в render может выглядеть как «render всё ещё владеет
  стрелкой»] → это осознанный мостик до `arrow-tuner-module`: потребители
  семьи вне `editor/` — тулбар и окно — переключатся на `arrow` в том
  change'е, и re-export схлопнется вместе с ними. Скидывать пути прямо
  здесь значило бы смешивать в один change два переезда.
- [Тестовая зависимость `arrow → render` в пиксельном тесте силуэта] →
  цикла модулей в продакшн-коде нет; тест гоняет полный путь отрисовки
  `render_composite → arrow::draw_arrow`, что и проверяет силуэт «как на
  экране». Выделение пиксельного теста в отдельный крейт/модуль дало бы
  ложную независимость.
- [TCC сессионного билда] → live-проверка стрелки (рисование, хит-тест,
  тюнер под `MINISNAP_ARROW_DEBUG`) при барьере помечается «ждёт ручной
  проверки»; юнит-тесты силуэта остаются страховкой регрессии.

## Migration Plan

Два коммита по tasks.md (модуль + семья → hit_test/чистый render),
каждый зелёный по `cargo build && cargo clippy && cargo test`; третий —
domain-skills pass. Откат — обратный порядок коммитов; публичных API вне
крейта нет.

## Open Questions

(нет)
