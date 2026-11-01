# Tasks

## 1. Клампы кропа — одна функция в документе

- [ ] 1.1 В `src/editor/mod.rs` появляется `clamp_crop_rect(rect, width, height) -> Rect` (standardize + прижатие к границам изображения); `apply_cmd::Crop` зовёт её вместо инлайн-клампа (поведение идентично). `commit_frame_edit` в `editor_window.rs` тоже зовёт её — свой инлайн-кламп (:1794–1799) удаляет, оставляя за собой только округление размера и порог `< 1`. Юнит-тесты клампа рядом с тестами `apply_cmd`. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — кроп и ресайз в редакторе работают как раньше.

## 2. Tool — тип редакторского домена

- [ ] 2.1 `Tool` переезжает из `editor_toolbar.rs` в `src/editor/mod.rs`; тулбар реэкспортирует его (`pub use`), кнопочная метаданные (`symbol`/`name`/`tag`/`all`) остаются в тулбаре. Все использования (`editor_window.rs`, `editor_toolbar.rs`) продолжают разрешаться. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — кнопки инструментов в полосе работают как раньше.

## 3. Словарь драга — в editor/drag.rs

- [ ] 3.1 Создать `src/editor/drag.rs` (декларация в `src/editor/mod.rs`): переезд типов `Drag`, `FrameSpot`, `FrameEdit`, `PendingCallout`, констант `MIN_DRAG_PX`, `CALLOUT_TEXT_OFFSET_PX` и функций `point_dist`, `draft_is_real`, `frame_rect`, `callout_anchor`, `spot_points`, `spot_at` (поиск ручки с допуском-параметром — из `frame_spot_at` окна). Окно импортирует словарь; ivars и структура обработчиков пока прежние. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — драг, коллаут, рамка работают как раньше.

## 4. DragMachine — тонкие обработчики окна

- [ ] 4.1 В `editor/drag.rs` появляется `DragMachine { drag, callout, frame }` с interface по design (`down`/`drag_to`/`up`/`preview`, `begin_frame`/`end_frame`/`current_frame`/`spot_points`/`commit_frame`, `pending_callout_anchor`/`commit_callout_text`/`cancel_callout_text`); `TextTarget`, `DEFAULT_TEXT_SIZE_PX`, `Signal`/`Select`/`DownCtx`/`DownOutcome`/`UpOutcome`/`Preview` — словарь машины. В `editor_window.rs` ivars `drag`/`pending_callout`/`frame_edit` схлопываются в `machine: RefCell<DragMachine>`; `handle_mouse_down`/`handle_mouse_up`/`handle_mouse_dragged`, рамочные и коллаутные методы становятся тонкими (pixel_point + пересылка + chrome); сборка коллаута и клампы уходят из окна. Поведение идентично, включая порядок «коммит текста → клик» и дегенеративную рамку. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — полный проход: рисование/перенос/концы, коллаут в две фазы, кроп/ресайз с рамкой, просмотр с переносом окна, undo.

## 5. Юнит-тесты машины

- [ ] 5.1 Тесты interface `DragMachine` без окна и без CGImage: клик короче `MIN_DRAG_PX` не создаёт элемент (все виды черновиков), тяга коммитит `AddElement` с переданным id; перенос и конец элемента — команда только после реальной тяги; правила выделения (Body/End/пусто); сигнал `DragWindow` в просмотре; `draft_is_real`; `frame_rect`/`spot_at` (отрицательная тяга стандартизуется, Center переносит целое); рамка через interface: begin → тяга → up оседает → commit клампится в кадр, дегенеративная остаётся; коллаут: up → фаза ввода → `commit_callout_text` собирает `Callout` у якоря, пустой текст сбрасывает фазу. Проверка: `cargo test` — новые тесты зелёные, живое окно не требовалось.

## 6. Domain-skills pass

- [ ] 6.1 Обновить `.zcode/skills/minisnap-editor/SKILL.md` (карта модулей: `editor/drag.rs` — машина драга и её interface; инварианты: где живут MIN_DRAG_PX, фазы коллаута, клампы кропа) и `.zcode/skills/minisnap-architecture/SKILL.md` (карта модулей). Проверка: текст скиллов соответствует коду.

## Workflow follow-up

- Архивировать change (`/opsx:archive`), обновить GLOSSARY.md (термин «машина драга» — снять пометку «план»).
