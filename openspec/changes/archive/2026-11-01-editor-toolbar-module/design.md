# Design

## Context

`src/editor_window.rs` — 4128 строк, тулбар и хром занимают ~1200 из них: константы
(:41–228), `WidthSliderView` (:478–739), hover/plates/поповер (:2755–3143), фабрики
(:3269–3402), сборка ряда в `EditorWindow::new` (:3788–3935), плюс выравнивание
traffic lights (:2696–2727) и восемь тулбарных полей `EditorIvars`. Решения
пользователя фиксируют целевую форму: `ToolbarView` — собственный objc2-view,
вход `set_state(ToolbarState)`, выход `ToolbarEvent` через один колбэк, plates —
в общий `src/plates.rs`. Этот design закрепляет форму interface'ов и
промежуточные состояния тасков.

## Module

### `src/plates.rs` — общая механика подсвеченных подложек

Ездит из `editor_window.rs`: имена слоёв (`PLATE_LAYER_NAME`, `DISC_LAYER_NAME`),
размеры (`TOOLBAR_PLATE_SIZE`, `SWATCH_DISC`), цвета (HOVER_BG, SWATCH_RING,
SWATCH_RING_HOVER), длина кроссфейда `PLATE_TRANSITION` с обёрткой
`animate_plate_changes`, поиск именованного подслоя и проход центрирования
подслоёв по устаканившимся границам кнопки (сегодня `named_sublayer`,
`toolbar_plate`, `swatch_disc`, `center_toolbar_layers`).

Форма: свободные функции над `&NSButton` — `named_sublayer`, `center_layers`
(параметризована таблицей «имя слоя → сторона»), константы в pub. Потребители:
`ToolbarView` (кнопки тулбара + свотч) и код палитры, который остаётся в
`editor_window.rs`. Никакого состояния — механика без владельца.

### `src/editor_toolbar.rs` — ToolbarView и interface

Ездит: константы полосы (TOOLBAR_HEIGHT, BAR_*, FILE_LABEL_WIDTH, ICON_TINT,
теги PAGE_PREV/PAGE_NEXT/COLOR_BUTTON, TOOLBAR_BUTTON_SIZE, BAR_SPACING,
BAR_ZONE_SPACING, ACTIVE_WASH, слайдерные константы), enum `Tool` (он «in toolbar
order» и несёт кнопочную метаданные: symbol/name/tag), `ToolbarPanel` (растворяется
в `ToolbarView` — тот сам перехватывает клики по свободному месту полосы и тянет
окно), фабрики (symbol_button, symbol_image, bar_separator, icon_button,
callout_icon), `WidthSliderView` целиком, сборка ряда, hover-проход, поповер
палитры, дебаг-тюнер стрелки (`ARROW_PARAM_SPECS`, `arrow_tuner_panel`,
`arrow_params_prompt` — его поле `arrow_param_rows` из восьми переезжающих).

`ToolbarView` — подкласс NSView с ivars: `tool_buttons`, `file_label`,
`color_button`, `color_popover`, `width_slider`, `arrow_param_rows`, `min_width`,
колбэк событий, tracking areas. Он же — полоса-панель (суперролева `ToolbarPanel`):
mouseDown по свободному месту → `performWindowDragWithEvent`, mouseEntered/Exited/
Moved → hover-проход. Часть его ivars — `Retained`-слоты, заполняемые в `new`.

**Отклонение 1 (форма сохранена: module).** Тюнер стрелки в списке переезжающих
диапазонов не назван, но его ivar `arrow_param_rows` входит в восемь тулбарных
полей, а панель — хром окна. Тюнер переезжает в `editor_toolbar.rs` целиком
(спека таблицы, панель, промпт, обработчики), его события идут через общий
`ToolbarEvent`. Без этого окно держало бы девятое, гибридное поле.

### ToolbarState — вход

```rust
pub struct ToolbarState {
    pub tool: Option<Tool>,
    pub stroke_width: f64,
    pub color: Rgba,
    pub can_undo: bool,
    pub can_redo: bool,
    pub file_label: String,
}
```

**Отклонение 3 (форма сохранена: interface).** Поле `color` в решениях не
перечислено (список снимка: инструмент, ширина, can_undo/can_redo,
enabled-правила, метка файла), но диск свотча — тулбарный контрол, и без
цвета в снимке окну понадобился бы второй вход (`show_color`), что ломало бы
«set_state — единственный вход». Цвет поехал в снимке; редукция проводит его
насквозь (не вычисляет).

Единственный вход. `set_state` делает всё state-зависимое: метку файла, подсветку
активного инструмента, цвет свотча, значение слайдера (setDoubleValue — равное
значение слайдер не переигрывает, петли событий нет), выравнивание traffic lights
и центрирование plates (сегодня оба прохода и так стартуют после каждого
display_shot/redraw). Семантика «слайдер следует за выделением» сохраняется:
окно считает ширину при сборке состояния и кладёт её в `stroke_width`;
`can_undo/can_redo` вычисляются, но кнопок под них пока нет — это interface
впрок, вариативная часть снимка.

**Отклонение 2 (форма сохранена: interface).** В решениях названы
«enabled-правила» и действия «undo/redo/copy/save/…». Кнопок этих действий в
тулбаре нет (undo/redo/copy — клавиатурные), поэтому variants на них в
`ToolbarEvent` не заводятся: мёртвые варианты нельзя покрыть тестом, а enum
расширится одним вариантом в день появления кнопки. «Enabled-правила» в снимке —
это `can_undo`/`can_redo`; других правил доступности контролов сегодня нет.

### ToolbarEvent — выход

```rust
pub enum ToolbarEvent {
    ToolToggled(Tool),          // клик по кнопке инструмента
    WidthChanged(f64),          // тяга слайдера
    PagePrevious,
    PageNext,
    PresetSelected(Rgba),       // свотч палитры
    ArrowParamChanged { index: usize, value: f64 },
    ArrowParamReset(usize),
    ArrowParamsReset,           // «reset» всего тюнера
}
```

Один колбэк: `ToolbarView::new` принимает `on_event: impl Fn(ToolbarEvent) +
'static`; замыкание держит `WeakId<EditorView>` (не Retained — без цикла
view→toolbar→view), при событии апгрейдится и зовёт
`EditorView::handle_toolbar_event`. Target/action остаётся внутренним делом
`ToolbarView` (все кнопки таргетят его; он переводит селекторы в события) —
наружу больше не торчит.

Клик по свотчу цвета — чисто тулбарная механика (открыть/закрыть поповер) —
события не порождает. Выбор пресета — да: он меняет документ. Копирование промпта
тюнера — чистый clipboard, событий не порождает.

### Чистая редукция

```rust
pub fn toolbar_state(
    doc: &EditorDoc,
    file_label: String,
    tool: Option<Tool>,
    stroke_width: f64,
    selection: Option<u64>,
) -> ToolbarState
```

Считает can_undo/can_redo из `EditorDoc::can_undo/can_redo` (уже есть), ширину
слайдера: у выделенного штрихового элемента — её кламп в диапазон слайдера, иначе
текущая `stroke_width`. `stroke_width_of` переезжает следом (единственный
потребитель — редукция). Тесты — чистые: `EditorDoc::new` от битмап-контекста
1×1, элементы, вызов, ассерты. Без окна.

Вызовы: окно собирает состояние в `redraw()` (сегодня там же `sync_width_slider`)
и после операций, меняющих тулбарную видимость, — единая точка пуша вместо
разбросанных `update_tool_highlight`/`update_color_swatch`/`setStringValue`.

## Interface (стык с окном)

- `ToolbarView::new(mtm, initial: ToolbarState, on_event) -> Retained<Self>` —
  строит полосу целиком, применяет начальное состояние.
- `view()` — бэкинг для `contentView().addSubview` (сам ToolbarView и есть
  полоса-панель).
- `min_width() -> f64` — минимальная ширина контента (окно кладёт её в
  `setContentMinSize` и читает в `refit_window`).
- `set_state(ToolbarState)` — единственный вход.
- `TOOLBAR_HEIGHT` — pub-константа, часть геометрического договора (окно считает
  высоту контента).

Что окно больше не делает: не держит тулбарных ivars (в `EditorIvars` вместо
восьми полей остаётся один слот `toolbar: RefCell<Option<Retained<ToolbarView>>>`),
не владеет tracking areas полосы, не является delegate поповера, не знает
именованых подслоёв. Порядок зависимостей один: окно → тулбар → plates.

## Seam (промежуточные состояния тасков)

Каждый таск — зелёный билд и живое окно. Переезд идёт по шву в три шага,
поэтому промежуточно существуют обратные зависимости, которые поздние таски
снимают:

- Таск 2: `editor_toolbar.rs` появляется с константами, `Tool`, фабриками,
  `ToolbarView` и сборкой; hover-проход кнопок уезжает сюда же (кнопки стали его
  ivars — с ними едет и их подсветка). Слайдер и поповер ещё в
  `editor_window.rs`: свотч и слайдер временно таргетят `EditorView` (старые
  селекторы), `ToolbarView::new` принимает временный параметр-цель; остаётся
  переходная зависимость `editor_toolbar → editor_window::WidthSliderView`.
- Таск 3 снимает оба переходных: `WidthSliderView`, поповер, палитрный hover и
  тюнер переезжают, свотч/слайдер/пресеты таргетят `ToolbarView`, временный
  параметр конструктора умирает.
- Таск 4 фиксирует финальный interface: `set_state` + финальный `ToolbarEvent`,
  колбэк с `WeakId`, `EditorIvars` теряет последнее тулбарное поле.

Поведенчески каждый шаг не меняет окно; риск — только в AppKit-проводке
(delegate поповера, tracking areas, re-entrancy `performClose` →
`popoverDidClose`), поэтому live-проверка — в каждом таске.

## Грабли, которые переносим с собой (не чиним здесь)

- Re-entrancy: `performClose` поповера ре-энтерит `popoverDidClose` — слот
  забирается до закрытия (сегодня :3033, переедет как есть); слайдер обязан
  держать tracking loop в `mouseDown:`, иначе macOS 26 переигрывает пресс в
  драг окна.
- Тулбарная полоса — титлбар окна: `setMovable(false)` в окне, сборка не меняет.
- Выравнивание traffic lights — после refit (титлбар релеится на ресайзе);
  в новой форме его делает `set_state`, поэтому вызов из `display_shot` после
  `refit_window` сохраняет порядок.

## Verification

- Каждый таск: `cargo build`, `cargo clippy`, `cargo test`; затем live: собрать
  release, открыть редактор (`MINISNAP_DEBUG_OPEN_EDITOR`), скриншотом сверить
  полосу (hover, активный инструмент, поповер, слайдер, метка), спрятать окно.
- Таск 5 добавляет юнит-тесты редукции (can_undo/can_redo, ширина по выделению,
  метка) — без окна.
- Правила «Working with a running app»: экземпляры различать по пути бинарника,
  окна прятать после каждой проверки, worktree-инстансов не оставлять.
