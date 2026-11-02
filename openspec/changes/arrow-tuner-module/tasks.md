# Tasks

## 1. Модуль `src/editor_arrow_tuner.rs` — панель-владелец

- [ ] 1.1 Создать `src/editor_arrow_tuner.rs` (`mod editor_arrow_tuner;` в
  `main.rs`): построчный переезд из `editor_toolbar.rs` — `ArrowParamSpec`
  + `ARROW_PARAM_SPECS` (типы — `crate::editor::arrow::ArrowParams`),
  `arrow_params_prompt`, геометрия и сборка панели; новый objc2-класс
  `ArrowTunerPanel` (наследник `NSBox`) с ivars `rows`/`on_event` держит
  четыре переехавших селектора, target/action всех контролов замыкаются
  на панель, события — `ToolbarEvent::ArrowParam*` через on-event-колбэк
  (`arrow_tuner_panel(mtm, on_event) -> Retained<ArrowTunerPanel>`).
  В `editor_toolbar.rs`: семья тюнера, четыре селектора и ivars
  `arrow_param_rows` удаляются, doc-комментарии вариантов `ArrowParam*`
  указывают на спеки тюнера; в `editor_window.rs` хук строит панель из
  нового модуля с клоном замыкания на `handle_toolbar_event`, обработчики
  берут `ARROW_PARAM_SPECS` из модуля тюнера. Тест промпта переезжает в
  новый модуль. Проверка: `cargo build`, `cargo clippy`, `cargo test`;
  live — панель тюнера над канвасом под `MINISNAP_ARROW_DEBUG` (если TCC
  позволит; иначе «ждёт ручной проверки»).

## 2. Семья стрелки — пути напрямую через `arrow`, re-export схлопнут

- [ ] 2.1 Иконка коллаута в `editor_toolbar.rs` и три обращения
  обработчиков `ArrowParam*` в `editor_window.rs` переходят с путей
  `render::` на `crate::editor::arrow`; re-export
  `pub(crate) use super::arrow::{...}` в `render.rs` удаляется — в
  `render.rs` не остаётся ничего про семью стрелки. Проверка:
  `cargo build`, `cargo clippy`, `cargo test`; live — стрелка рисуется,
  тюнер меняет живую форму (если TCC позволит; иначе «ждёт ручной
  проверки»).

## 3. Domain-skills pass

- [ ] 3.1 Обновить `.zcode/skills/minisnap-editor/SKILL.md` (таблица
  модулей: строка `src/editor_arrow_tuner.rs` — debug-панель-владелец,
  свои rows и селекторы, наружу `ToolbarEvent::ArrowParam*`; строка
  `editor_toolbar.rs` без тюнера и `arrow_param_rows`; буллет про
  `MINISNAP_ARROW_DEBUG` — панель — свой objc2-класс, варианты
  `ArrowParam*` — собственность тюнера) и строки
  `editor_toolbar.rs`/`editor_arrow_tuner.rs` в
  `.zcode/skills/minisnap-architecture/SKILL.md`. Проверка: текст скиллов
  соответствует коду.
