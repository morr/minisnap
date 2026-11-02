# Tasks

## 1. Модуль `src/editor/arrow.rs` — семья стрелки

- [x] 1.1 Создать `src/editor/arrow.rs` (`pub(crate) mod arrow;` в
  `editor/mod.rs`): построчный переезд из `render.rs` — `ArrowParams`
  (+`DEFAULT`), глобал `ARROW_PARAMS` с `arrow_params`/`set_arrow_params`,
  `ArrowFrame`, `arrow_outline(_with)`, `quad`, `round_corners`, `pt_dist`,
  `arrow_gradient`, `draw_arrow`; в `render.rs` — re-export
  `pub(crate) use super::arrow::{arrow_params, draw_arrow, set_arrow_params, ArrowParams};`,
  `draw_element` зовёт `arrow::draw_arrow`. Тесты силуэта
  (`arrow_matches_the_monosnap_shape`, `arrow_outer_edge_bows_out`,
  `arrow_params_reshape_the_outline`) переезжают в `arrow.rs`. Проверка:
  `cargo build`, `cargo clippy`, `cargo test`; live — стрелка рисуется в
  редакторе (если TCC позволит; иначе «ждёт ручной проверки»).
  *(build/clippy/test зелёные: 128 тестов, три теста силуэта едут из
  `editor::arrow::tests`; clypy-профиль arrow.rs/render.rs чист, прочие
  предупреждения — прежние dead_code/deprecated. Re-export дополнительно
  держит `arrow_outline` — единственный его потребитель `hit_test`
  переходит на `arrow` в таске 2. Live: дебаг-сборка PID 24300,
  `MINISNAP_DEBUG_OPEN_EDITOR` + сессия с командой AddElement(Arrow)
  фиолетового цвета — на скриншоте окна редактора ровно 0 совпадающих
  пикселей до запуска и ~2015 (downscale) после, bounding box 796×656 —
  диагональный след стрелки, не свотч тулбара; приложение погашено по
  PID, временная сессия удалена)*

## 2. Зависимость `hit_test → arrow`, чистый render

- [x] 2.1 В `hit_test.rs` оба места (`Arrow` и хвост коллаута) берут
  `arrow_outline` из `super::arrow`; `callout_tail` остаётся за
  `render::`. В `render.rs` не остаётся ничего про форму стрелки:
  только композит (`callout_tail`, `pixelate_region`, `bitmap_context`,
  `render_composite`/`draw_elements`) и его тесты. Проверка:
  `cargo build`, `cargo clippy`, `cargo test`; live — хит-тест стрелки
  (тяга концов, перенос), тюнер под `MINISNAP_ARROW_DEBUG` меняет форму
  живой стрелки (иначе «ждёт ручной проверки»).
  *(build/clippy/test зелёные: 128 тестов, hit_test-тесты стрелки едут
  без правок; `arrow_outline` снят с re-export в render — внешний
  потребитель один и он мигрировал; в render.rs из семьи стрелки не
  осталось ни строки, re-export — только то, что ещё живёт через
  `render::` пути. Live: без `MINISNAP_ARROW_DEBUG` — стрелка целиком
  (2015 px, bbox 199×164); с `MINISNAP_ARROW_DEBUG` — панель тюнера
  перекрыла левую часть стрелки (1891 px, bbox 174×153, левый край
  сдвинут на 25 px downscaled): панель строится и читает
  `render::arrow_params` через re-export, стрелка рендерится как раньше,
  логи обоих запусков чистые. Интерактив — тяга концов (Accessibility,
  синтетика ловила SIGTRAP в раунде 1) и «крутилка меняет живую стрелку»
  (нужны клики по слайдерам): ждёт ручной проверки; геометрия покрыта
  юнит-тестами `editor::arrow` и `editor::hit_test`)*

## 3. Domain-skills pass

- [ ] 3.1 Обновить `.zcode/skills/minisnap-editor/SKILL.md` (таблица
  модулей: строка `src/editor/arrow.rs` — параметризация, силуэт,
  отрисовка, глобал крутилок; строка `render.rs` — чистый композит без
  стрелки; зависимость `hit_test → arrow` за геометрию, `callout_tail`
  остаётся в render; буллет про `MINISNAP_ARROW_DEBUG` — панель крутит
  `arrow::ARROW_PARAMS` через re-export `render::`) и строку
  `src/editor/render.rs` в `.zcode/skills/minisnap-architecture/SKILL.md`.
  Проверка: текст скиллов соответствует коду.
