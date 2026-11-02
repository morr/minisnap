# Design

## Context

`src/editor_toolbar.rs` (1987 строк на момент планирования) держит, помимо
тулбара, весь debug-тюнер стрелки: таблицу спеков `ArrowParamSpec`/
`ARROW_PARAM_SPECS` (12 строк-крутилок над `ArrowParams`), текст промпта
`arrow_params_prompt` (копирует настроенные значения с инструкцией для
кодинг-агента в буфер), сборку панели `arrow_tuner_panel` (NSBox со
слайдерами; NSBox выбран не случайно — на этой macOS inline-hosted контролы
перестают композитить внутри view с ручным слоем, комментарий в коде),
четыре селектора в классе `ToolbarView` (`arrowParamChanged:`,
`arrowTunerReset:`, `arrowParamReset:`, `arrowPromptCopy:`) и ivars-поле
`arrow_param_rows`. Панель строится хуком в `editor_window.rs`
(`show_shot`-ветка сборки окна) под `MINISNAP_ARROW_DEBUG` и таргетит
`ToolbarView`: слайдеры и кнопки панели — селекторы тулбара, rows живут в
ivars тулбара, события уходят в окно через общий колбэк.

Потребители семьи стрелки после пункта 2 (`arrow-render-module`): тюнер
(спеки типизированы `ArrowParams`, промпт читает `arrow_params`),
иконка коллаута в тулбаре (`render::draw_arrow`), обработчики
`ToolbarEvent::ArrowParam*` в окне (`arrow_params`, `set_arrow_params`,
`ArrowParams::DEFAULT` через re-export `render::`), `hit_test` (уже на
`arrow` напрямую), `render::draw_element` (уже на `arrow` напрямую).
Re-export `pub(crate) use super::arrow::{...}` в `render.rs` — мостик,
который design `arrow-render-module` явно обяжал схлопнуть в этом change'е.

## Goals / Non-Goals

**Goals:**

- Debug-тюнер живёт в одном модуле `src/editor_arrow_tuner.rs` со своим
  view-owner'ом: панель — самодостаточный objc2-объект, держит свои rows,
  таргетит себя и выдаёт наружу готовые события.
- `editor_toolbar.rs` — только тулбар и хром: ни спеков, ни промпта, ни
  панели, ни тюнерных селекторов и ivars; из семьи стрелки тулбар знает
  только `arrow::draw_arrow` для иконки коллаута.
- Тестируемость не падает: тест промпта переезжает вместе с промптом.
- Наблюдаемое поведение не меняется.

**Non-Goals:**

- Не менять внешний шов `ToolbarView` (`set_state(ToolbarState)`/
  `ToolbarEvent`): варианты `ArrowParam*` остаются в enum'е, окно применяет
  их как раньше.
- Не менять нормативное поведение и спеки (`skip_specs`).
- Не трогать швы прошлого раунда: машина драга, `present_target`/
  `SurfaceMap`, `capture::geometry`, модуль `overlay_hud`, модуль
  `editor/arrow`.
- Не выносить `ToolbarEvent` в общий модуль: ради одного импорта тюнера
  размывать шов не нужно.

## Decisions

1. **Файл модуля — `src/editor_arrow_tuner.rs`, `mod` в `main.rs`**,
   рядом с `editor_toolbar.rs`, не внутри `editor/`: панель — хром окна
   над канвасом (как тулбар), а не документ/рендер; план термина в
   GLOSSARY.md называет ровно этот путь.

2. **Владелец панели — свой objc2-класс `ArrowTunerPanel`, наследник
   `NSBox`.** База остаётся `NSBox` по той же причине, что и раньше:
   классическая отрисовка бокса не ломает композитинг inline-hosted
   контролов. Ivars класса: `rows`
   (`RefCell<Vec<(Retained<NSSlider>, Retained<NSTextField>)>>`) и
   `on_event` (`RefCell<Option<Box<dyn Fn(ToolbarEvent)>>>`). Четыре
   селектора переезжают с `ToolbarView` на панель построчно; вместо
   `self.ivars().arrow_param_rows` — свои ivars, вместо общего колбэка —
   свой `emit` (по образцу тулбарного). Сигнатура конструктора:
   `arrow_tuner_panel(mtm, on_event: impl Fn(ToolbarEvent) + 'static)
   -> Retained<ArrowTunerPanel>` — панель строится с колбэком, как
   `ToolbarView::new`; `setBoxType`/размер/строки — как раньше, но
   target/action всех контролов замыкаются на саму панель, и `rows`
   записываются в её ivars.

3. **События — готовые `ToolbarEvent::ArrowParam*` через on-event-колбэк.**
   Enum не меняется (шов); тюнер импортирует `ToolbarEvent` и помощники
   `icon_button`/`symbol_image` из `editor_toolbar` — односторонняя
   зависимость `editor_arrow_tuner → editor_toolbar`. В окне хук строит
   панель с клоном существующего замыкания (`Weak<EditorView>` →
   `handle_toolbar_event`), так что события тюнера приходят в окно тем же
   путём, что и тулбарные. Альтернатива — таргетить панелью `ToolbarView`
   и переэммитить — отклонена: держала бы тюнерное состояние в ivars
   тулбара, ради искоренения которого затевается change.

4. **`ARROW_PARAM_SPECS` — `pub(crate)` в модуле тюнера; окно применяет
   события через неё как раньше** (импорт меняет только путь). Типы —
   `ArrowParams` из `crate::editor::arrow` (пункт 2 цепочки применён,
   ветка «продолжай с render::ArrowParams» из решений не актуальна).
   `arrow_params_prompt` — приватная функция модуля. Уточнение, всплывшее
   при переезде: текст промпта велел бекапить и править
   `ArrowParams::DEFAULT` в `src/editor/render.rs` — после пункта 2
   дефолты живут в `src/editor/arrow.rs`, поэтому оба упоминания пути в
   тексте промпта переезжают актуализированными (это вывод отладочного
   инструмента, а не продуктовое поведение; тест промпта пути не
   проверяет и остаётся зелёным).

5. **Re-export семьи стрелки в `render.rs` схлопывается** (п. 3–4).
   Оставшиеся потребители вне `editor/` переходят на `crate::editor::arrow`
   напрямую: иконка коллаута тулбара — `arrow::draw_arrow`, обработчики
   окна — `arrow::arrow_params`/`arrow::set_arrow_params`/
   `arrow::ArrowParams::DEFAULT`. В `render.rs` из семьи стрелки не
   остаётся ни строки — он чистый композит, как сформулировано в
   `arrow-render-module`.

6. **Тесты.** Промпт-тест
   (`arrow_prompt_carries_every_param_and_the_steps`) переезжает в
   `editor_arrow_tuner.rs` дословно. Уточнение к формулировке решений
   («тесты редукции и промпта тюнера переезжают вместе с ним»): редукция
   `toolbar_state` не переезжает — она остаётся чистым ядром тулбара, и её
   тесты остаются в `editor_toolbar.rs`; «переезжает» из тюнерного —
   только промпт-тест. Тестируемость не падает: количество и покрытие
   тестов сохраняются, проверка промпта остаётся юнит-тестом без окна.

## Risks / Trade-offs

- [Зависимость `editor_arrow_tuner → editor_toolbar` может читаться как
  «тюнер — часть тулбара»] → зависимость только на шов (`ToolbarEvent`) и
  иконные помощники; панель больше не делит с тулбаром ни состояния, ни
  селекторов. Перенос `ToolbarEvent` в общий модуль дал бы чистую
  топологию ценой churn в каждом импорте шва — сознательно не в этом
  change'е.
- [Панель — второй objc2-класс в домене редактора] → это цена
  «target/action замыкаются на панель»: владелец событий обязан быть
  ObjC-объектом. Класс маленький (4 селектора + emit + ivars), по образцу
  `WidthSliderView`/`ToolbarView`.
- [TCC сессионного билда] → live-проверка тюнера (панель строится,
  слайдеры двигают живую стрелку) при барьере помечается «ждёт ручной
  проверки»; юнит-тесты промпта и статическая проверка структуры окна
  остаются страховкой.

## Migration Plan

Три коммита по tasks.md (модуль → схлоп re-export → domain-skills pass),
каждый зелёный по `cargo build && cargo clippy && cargo test`; четвёртый —
архивация с GLOSSARY.md. Откат — обратный порядок коммитов; публичных API
вне крейта нет.

## Open Questions

(нет)
