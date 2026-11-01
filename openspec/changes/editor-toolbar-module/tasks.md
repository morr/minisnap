# Tasks

## 1. plates-подмодуль

- [x] 1.1 Создать `src/plates.rs`: переезд из `editor_window.rs` имён слоёв (`PLATE_LAYER_NAME`, `DISC_LAYER_NAME`), размеров (`TOOLBAR_PLATE_SIZE`, `SWATCH_DISC`), цветов (`HOVER_BG`, `SWATCH_RING`, `SWATCH_RING_HOVER`), `PLATE_TRANSITION` с `animate_plate_changes`, поиска именованного подслоя и прохода центрирования (параметризованная таблица «имя → сторона»). Декларация модуля в `main.rs`. Оба потребителя в `editor_window.rs` (кнопки тулбара и палитра) зовут `plates::*`; дубликаты из окна удалены. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — hover и подсветка кнопок/свотчей не изменились. *(build/clippy/test зелёные; live: полоса, метка, hover-подложка и акцент активного инструмента подтверждены скриншотами; поповер палитры и слайдер не доиграны — эмуляция упёрлась в активность пользователя (сработал захват ⌘⌥4), ждёт ручной проверки)*

## 2. Константы, фабрики и сборка в ToolbarView

- [x] 2.1 Создать `src/editor_toolbar.rs`: переезд констант полосы, enum `Tool`, `ToolbarPanel` (растворяется в `ToolbarView`), фабрик (symbol_button, symbol_image, bar_separator, icon_button, callout_icon) и класса `ToolbarView` с собственными ivars (кнопки, метка файла, min_width, слот колбэка). `ToolbarView` строит полосу целиком и является поверхностью-панелью (drag за свободное место, tracking areas, hover-проход и подсветка кнопок — его методы). Переходно: свотч и слайдер таргетят `EditorView` (старые селекторы), временный параметр-цель в конструкторе; `EditorIvars` отдаёт часть тулбарных полей, окно читает геометрию через `TOOLBAR_HEIGHT` и `min_width()`. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — полоса, hover, инструменты, листание работают как раньше. *(build/clippy/test зелёные; live нашёл регрессию — ряд уходил под нижнюю границу полосы, потому что полоса фреймилась после добавления стека и авторезайзинг размазывал его; фикс: фреймить полосу до добавления стека, геометрия после фикса сверена дампом фреймов (стек (85,4,383,24) внутри полосы (0,320,480,36)) и AX-деревом (все 14 контролов, правильные X-шаги); пиксельная сверка окна — screencapture перестал брать окна сессии, ждёт ручной проверки; инстанс с фиксом оставлен запущенным (pid 80233, редактор на /tmp/minisnap-verify/shot.png))*

## 3. WidthSliderView, поповер палитры и тюнер

- [ ] 3.1 Переезд `WidthSliderView` в `editor_toolbar.rs` (снятие переходной зависимости): слайдер таргетит `ToolbarView`, ширина уходит окну через колбэк. Поповер палитры, палитрный hover, `update_color_swatch` и обработка клика свотча — в `ToolbarView` (delegate поповера — он же; re-entrancy `performClose` сохраняется). Дебаг-тюнер (`ARROW_PARAM_SPECS`, панель, промпт, обработчики) переезжает целиком со своим полем `arrow_param_rows`. Временный параметр конструктора удалён. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — слайдер тянет ширину, палитра открывается/выбирает цвет, hover свотчей работает, тюнер под `MINISNAP_ARROW_DEBUG` крутится.

## 4. ToolbarEvent + set_state

- [ ] 4.1 Зафиксировать interface: `ToolbarState` (tool, stroke_width, can_undo, can_redo, file_label), финальный `ToolbarEvent` (ToolToggled, WidthChanged, PagePrevious, PageNext, PresetSelected, ArrowParamChanged, ArrowParamReset, ArrowParamsReset), один колбэк с `WeakId<EditorView>` вместо временной цели, `set_state` как единственный вход (метка, подсветка, свотч, слайдер, traffic lights, центрирование plates). Окно пушит состояние из `redraw()` и после смены снимка/undo; `sync_width_slider`, `update_tool_highlight`, `update_color_swatch`, `align_traffic_lights` и прямые правки метки из окна удалены. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live — полный проход: листание, инструменты, цвет, ширина, undo/redo, метка файла.

## 5. Чистая редукция + юнит-тесты

- [ ] 5.1 `toolbar_state(doc, file_label, tool, stroke_width, selection) -> ToolbarState` — чистая функция в `editor_toolbar.rs` (can_undo/can_redo из `EditorDoc`, ширина слайдера по штриховому выделению через переехавший `stroke_width_of`, кламп в диапазон слайдера); окно собирает состояние только через неё. Юнит-тесты без окна: undo/redo-флаги по журналу, ширина по выделению штрихового/нештрихового/ничего, кламп диапазона, метка. Проверка: `cargo test` покрывает редукцию; live-проход редактора не изменился.

## 6. Domain-skills pass

- [ ] 6.1 Обновить `.zcode/skills/minisnap-editor/SKILL.md` (карта модулей: `editor_toolbar.rs`, `plates.rs`; инварианты: interface ToolbarView, где живёт механика plates/hover) и `.zcode/skills/minisnap-architecture/SKILL.md` (карта модулей — два новых файла верхнего уровня). Проверка: текст скиллов соответствует коду.

## Workflow follow-up

- Архивировать change (`/opsx:archive`), обновить GLOSSARY.md (ToolbarView, ToolbarState, ToolbarEvent, plates — снять пометку «план»).
