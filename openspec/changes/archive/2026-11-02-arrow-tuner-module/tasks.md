# Tasks

## 1. Модуль `src/editor_arrow_tuner.rs` — панель-владелец

- [x] 1.1 Создать `src/editor_arrow_tuner.rs` (`mod editor_arrow_tuner;` в
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
  *(build/clippy/test зелёные: 128 тестов, промпт-тест едет из
  `editor_arrow_tuner::tests`, clippy по тронутым файлам чист, прочие
  предупреждения — прежние dead_code/deprecated/unused_unsafe; попутно из
  импорта окна убраны давно мёртвые `STROKE_MIN`/`STROKE_MAX`. Live
  (debug-билд, TCC сессии granted): A/B на белом снимке 800×600 — с
  `MINISNAP_ARROW_DEBUG` в зоне панели (8,44)–(302,358) pt окна 89.6%
  тёмных пикселей + 9.6% светлых (чёрный бокс 0.7 с белым текстом),
  центр канвы чисто белый; без env та же зона — 0 тёмных, 100% светлых.
  Интерактив — движение слайдеров меняет живую стрелку, reset/prompt:
  ждёт ручной проверки (синтетические клики по контролам ненадёжны,
  раунд 1 ловил SIGTRAP); путь событий — тот же `handle_toolbar_event`,
  селекторы и редукция rows переехали построчно)*

## 2. Семья стрелки — пути напрямую через `arrow`, re-export схлопнут

- [x] 2.1 Иконка коллаута в `editor_toolbar.rs` и три обращения
  обработчиков `ArrowParam*` в `editor_window.rs` переходят с путей
  `render::` на `crate::editor::arrow`; re-export
  `pub(crate) use super::arrow::{...}` в `render.rs` удаляется — в
  `render.rs` не остаётся ничего про семью стрелки. Проверка:
  `cargo build`, `cargo clippy`, `cargo test`; live — стрелка рисуется,
  тюнер меняет живую форму (если TCC позволит; иначе «ждёт ручной
  проверки»).
  *(build/clippy/test зелёные: 128 тестов, clippy по тронутым файлам
  чист — предупреждения прежние. Live (debug-билд): сессия с командой
  AddElement(Arrow) фиолетового цвета на белом снимке 800×600 — на
  скриншоте окна 23239 пурпурных пикселей (2x): путь
  `render::draw_element → arrow::draw_arrow` и панель тулбара (иконка
  коллаута через `arrow::draw_arrow`) работают после перевода импортов;
  лог запуска чистый, приложение погашено по PID. Интерактивная проверка
  тюнера (слайдеры меняют живую форму) — ждёт ручной проверки. Из
  render.rs ушёл re-export семьи — остались только прямые вызовы
  композита `super::arrow::draw_arrow` (ещё пункт 2 цепочки), ни одного
  пути/типа семьи через `render::`)*

## 3. Domain-skills pass

- [x] 3.1 Обновить `.zcode/skills/minisnap-editor/SKILL.md` (таблица
  модулей: строка `src/editor_arrow_tuner.rs` — debug-панель-владелец,
  свои rows и селекторы, наружу `ToolbarEvent::ArrowParam*`; строка
  `editor_toolbar.rs` без тюнера и `arrow_param_rows`; буллет про
  `MINISNAP_ARROW_DEBUG` — панель — свой objc2-класс, варианты
  `ArrowParam*` — собственность тюнера) и строки
  `editor_toolbar.rs`/`editor_arrow_tuner.rs` в
  `.zcode/skills/minisnap-architecture/SKILL.md`. Проверка: текст скиллов
  соответствует коду. *(minisnap-editor: границы домена дополнены
  `editor_arrow_tuner.rs`; строка тулбара без «тюнера» и
  `arrow_tuner_panel`, с пометкой про `ArrowParam*` — emit'ит тюнер;
  новая строка `src/editor_arrow_tuner.rs` — `ArrowTunerPanel`,
  `arrow_tuner_panel`, `ARROW_PARAM_SPECS`, `ArrowParamSpec`,
  `arrow_params_prompt`; буллет env — варианты `ArrowParam*`
  собственность тюнера, re-export-мостики больше не упоминаются.
  references/window-mechanics.md: строка канвы — тюнер целиком в своём
  модуле; interface тулбара — Weak клонируется панели, тюнерные селекторы
  — внутреннее дело панели; секция тюнера переписана на
  панель-владельца. references/render-and-hit.md: секция крутилок —
  модуль и класс панели, дефолты пекутся в `src/editor/arrow.rs`.
  minisnap-architecture: строка тулбара без «дебаг-тюнера», новая строка
  `editor_arrow_tuner.rs`. Сверено с кодом: 5 pub-символов тюнера +
  класс `ArrowTunerPanel` в модуле, 2 `weak_view.clone()` в окне
  (тулбар + панель), в render.rs — только прямые вызовы
  `super::arrow::draw_arrow` композитом)*
