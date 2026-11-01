# Design

## Context

В `editor_window.rs` (2379 строк после выноса тулбара) стейт-машина драга —
сплошная чистая логика на типах `editor/mod.rs`: `Drag`/`FrameSpot`/`FrameEdit`/
`PendingCallout` (:108–169), мышиные обработчики (`handle_mouse_down` :822,
`handle_mouse_up` :1051, `update_drag` :1140), `draft_is_real` (:1260), рамочная
математика (`frame_rect` :2105, `frame_spot_points` :1652, `frame_spot_at`
:1670), коллаут-фазы (`callout_anchor` :2075, `pending_callout_anchor` :1380,
сборка в `commit_text_editor` :1519) и клампы кропа в `commit_frame_edit`
(:1794–1799), дублирующие `apply_cmd::Crop`. Решения фиксируют целевую форму:
чистый `src/editor/drag.rs` без AppKit-типов, события в пикселях на входе,
превью + `Command` + сигналы на выходе. Этот design закрепляет interface
машины, дом за домом, и промежуточные состояния тасков.

Текущие адреса в тексте — по состоянию после `editor-toolbar-module` (файл
похудел с 4128 до 2379 строк; в решениях пункта 2 адреса даны по старой
нумерации, соответствие: :1584→:822, :1813→:1051, :1902→:1140, :2022→:1260,
:3220→:2105, :2549→:1794).

## Module

### `src/editor/drag.rs` — машина драга

Без AppKit-типов. Зависимости: `crate::editor::{Command, Element, ElementKind,
Endpoint, Point, Rect, Rgba, Size}` (модуль документа), `crate::editor::hit_test`
(чистый), `crate::editor::text` (чистый замер, для сборки коллаута),
`crate::editor::Tool` (см. Отклонение 1). Направление одно: `drag.rs` →
модули документа; окно → `drag.rs`.

Состояние машины — три семьи, сегодня размазанные по трём ivars окна
(`drag`, `pending_callout`, `frame_edit`):

```rust
pub struct DragMachine {
    drag: Option<Drag>,            // тянущаяся мышь: Draw/Move/End/Frame
    callout: Option<PendingCallout>, // коллаут в фазе ввода текста
    frame: Option<FrameEdit>,      // режим кропа/ресайза
}
```

Словарь (переезжает из окна как есть): `Drag` (Draw/Move/End/Frame),
`FrameSpot`, `FrameEdit { resize, rect }`, `PendingCallout { base, tip, color,
size, width }`, константы `MIN_DRAG_PX`, `CALLOUT_TEXT_OFFSET_PX`,
`DEFAULT_TEXT_SIZE_PX`, `TextTarget` (payload сигнала «начать ввод текста»).

**Фазы коллаута (форма сохранена: interface).** «Явный тип фаз» — два явных
состояния машины: фаза 1 — черновик стрелки, обычный `Drag::Draw(Callout)`
внутри `drag`; фаза 2 — `callout: Some(PendingCallout)`. Переход (mouse-up
настоящей тяги: черновик расщепляется в `PendingCallout` + сигнал начать ввод),
обратный переход (пустой текст или Esc — фаза сбрасывается), расчёт якоря
(`callout_anchor` переезжает целиком) и сборка финального `ElementKind::Callout`
(`commit_text_editor` :1519–1541) — методы машины. Отдельный enum
`CalloutPhase::Drawing` не заводится: фаза 1 и так типизирована `Drag::Draw`,
фантомный вариант был бы дубликатом без состояния.

### Interface машины

```rust
pub struct DownCtx<'a> {
    pub tool: Option<Tool>,
    pub color: Rgba,
    pub stroke_width: f64,
    pub elements: &'a [Element],     // снимок для hit-теста
    pub frame: Option<Rect>,         // активная рамка, если режим поднят
    pub handle_tolerance: Point,     // допуск ручек рамки, px по осям
}

pub enum Select { Keep, Element(u64), Clear }
pub enum Signal {
    BeginText(TextTarget),  // TextTarget::New{..} | TextTarget::Existing{ id }
    DragWindow,             // пресс в просмотре по пустому месту
}
pub struct DownOutcome { pub selection: Select, pub signal: Option<Signal> }
pub struct UpOutcome { pub command: Option<Command>, pub signal: Option<Signal> }
pub struct Preview { pub hide: Option<u64>, pub extras: Vec<Element> }

impl DragMachine {
    pub fn down(&mut self, ctx: DownCtx, point: Point) -> DownOutcome;
    pub fn drag_to(&mut self, point: Point) -> bool;      // false — нет драга
    pub fn up(&mut self, point: Point, next_id: u64) -> Option<UpOutcome>;
    pub fn preview(&self, elements: &[Element]) -> Preview;

    // режим рамки
    pub fn begin_frame(&mut self, resize: bool, image: Size);
    pub fn end_frame(&mut self) -> bool;
    pub fn frame_active(&self) -> bool;
    pub fn current_frame(&self) -> Option<Rect>;          // с тянущейся рамкой
    pub fn spot_points(&self) -> Vec<(FrameSpot, Point)>;
    pub fn commit_frame(&mut self, image: Size) -> Option<Command>;

    // фаза коллаута
    pub fn pending_callout_anchor(&self, block: Size, image: Size) -> Option<Point>;
    pub fn commit_callout_text(&mut self, text: &str, image: Size, next_id: u64)
        -> Option<Command>;
    pub fn cancel_callout_text(&mut self);
}
```

**Готовая `Command` и id (форма сохранена: interface).** Решение требует
готовую `Command` на выходе, но id нового элемента — политика документа
(`EditorDoc::next_element_id` сканирует журнал вместе с redo-хвостом). Машина
не изобретает id: окно передаёт следующий свободный в `up` и
`commit_callout_text` (сегодня окно делает тот же запрос в тех же двух местах,
:1087 и :1550).

**Допуск ручек (форма сохранена: seam).** `frame_spot_at` (:1670) остаётся
view-геометрией окна только наполовину: перевод допуска через `view_scale`
делает окно (оно владеет масштабом показа), сам поиск — чистая функция машины
`spot_at(rect, point, tolerance)`; `DownCtx::handle_tolerance` — её вход в
`down`. `HANDLE_RADIUS_PT`, клоуны-курсоры и кламп ручек к карточке — остаются
в окне.

**Превью (форма сохранена: interface).** `preview` возвращает `hide` (id
элемента, который на время тяги не рисуется из документа) и `extras`
(перееханный/перетянутый элемент, черновик, стрелка коллаута фазы 2 — в порядке
отрисовки). Правило скрытия элемента под открытым field-редактором — знание
окна (`text_target`), в превью не попадает.

**Прочие чистые функции**, переезжают свободными: `point_dist`, `draft_is_real`,
`frame_rect` (математика рамки при тяне — :2105), `callout_anchor` (:2075),
`spot_points(rect)`, `spot_at(rect, point, tolerance)`.

### Клампы кропа — `src/editor/mod.rs`

```rust
/// Clamp a crop rect to the image bounds, standardized.
pub fn clamp_crop_rect(rect: Rect, width: f64, height: f64) -> Rect
```

`apply_cmd::Crop` зовёт её вместо инлайн-клампа (поведение идентично: тот же
standardize, тот же пересчёт, тот же `is_empty`-no-op). Машина зовёт её в
`commit_frame` — и добавляет только своё (округление размера до целых пикселей
и порог дегенеративности `< 1`, которых у `apply_cmd::Crop` нет; округление —
решение машины, не клампа). Дубликат :1794–1799 исчезает; окно после переезда
вообще не знает, как Crop себя клампит.

### Тонкие обработчики окна

- `handle_mouse_down` :822 → `commit_text_editor()` (field-редактор — окно),
  сборка `DownCtx` из ivars, `machine.down(ctx, pixel_point(event))`,
  применение `selection`, разбор `signal` (`BeginText` → `begin_text_edit`,
  `DragWindow` → redraw + `drag_window_with_mouse`).
- `handle_mouse_dragged`/`handle_mouse_up` → `pixel_point` + `drag_to`/`up`;
  `up` применяет `command` (`shot.apply`) и `signal` (коллаут-фаза 2).
- `update_drag`, `draft_is_real`, `frame_rect`, `handle_frame_drag`,
  `current_frame_rect`, `frame_spot_points`, `frame_spot_at`, `point_dist`,
  `MIN_DRAG_PX`, `CALLOUT_TEXT_OFFSET_PX`, `DEFAULT_TEXT_SIZE_PX`,
  `TextTarget`-сборка — из окна удаляются.
- `EditorIvars`: `drag`, `pending_callout`, `frame_edit` → один слот
  `machine: RefCell<DragMachine>`. `text_target`, `selection`, `tool`,
  `color`, `stroke_width` остаются в окне (field-редактор, маркеры, тулбар).
- `enter_frame_edit`/`exit_frame_edit`/`commit_frame_edit` становятся
  chrome-обвязкой вокруг `begin_frame`/`end_frame`/`commit_frame`; правило
  начального прямоугольника (кроп — весь кадр, ресайз — центральные 75%) — в
  `begin_frame`.

**Отклонение 1 (форма сохранена: module).** `Tool` переезжает из
`editor_toolbar.rs` в `src/editor/mod.rs`: машине нужен активный инструмент, а
зависимость «editor → editor_toolbar» инвертировала бы слойку, которую пункт 1
закрепил (окно → тулбар → editor). Кнопочная метаданные (`symbol`/`name`/
`tag`/`all`) остаются в тулбаре — это хром поверх типа-идентичности;
`editor_toolbar` реэкспортирует `Tool`, так что `editor_toolbar::Tool` и
`ToolbarEvent::ToolToggled(Tool)` продолжают разрешаться без изменений.

**Отклонение 2 (форма сохранена: interface).** Фаза ввода текста у машины —
только коллаут. Обычный текст (инструмент Text) собирается в `ElementKind::Text`
на стороне окна, как и сегодня: шесть адресов коллаута в решениях не включают
эту ветку `commit_text_editor`, а машина без состояния «ожидаю простой текст»
не нужна — цель ввода и так заморожена в `TextTarget` окна. Оборотная сторона:
правило «пустой ввод» для простого текста остаётся в окне, для коллаута —
уходит в машину (там и его состояние).

**Отклонение 3 (форма сохранена: interface).** Оборонительная ветка
`commit_text_editor` «`pending_callout` отсутствует → вырожденный коллаут в
точке клика» (:1522) не переезжает: она недостижима (фаза 2 ставится до
открытия редактора и снимается вместе с ним), а `commit_callout_text` при
отсутствующей фазе возвращает `None`. Наблюдаемого кода за ней нет.

## Seam (промежуточные состояния тасков)

Каждый таск — зелёный билд и живое окно. Переезд в четыре шага:

- Таск 1 (клампы) и таск 2 (`Tool`) независимы от машины: чистая функция в
  `mod.rs` с двумя зовущими, переезд типа с реэкспортом.
- Таск 3: `editor/drag.rs` появляется как словарь — типы, константы и чистые
  функции (`Drag`, `FrameSpot`, `FrameEdit`, `PendingCallout`, `draft_is_real`,
  `frame_rect`, `callout_anchor`, `spot_*`, `point_dist`, `MIN_DRAG_PX`);
  окно пользуется ими по импорту, ivars и структура обработчиков пока прежние.
  Промежуточное состояние: типы без владельца-машины.
- Таск 4 снимает его: `DragMachine` забирает три семьи состояния, обработчики
  окна становятся тонкими, ivars схлопываются в один слот.
- Таск 5 покрывает interface тестами; таск 6 — скиллы.

Поведенчески каждый шаг не меняет окно; риск — в переподключении обработчиков
(таск 4), поэтому live-проверка — в каждом таске переезда.

## Грабли, которые переносим с собой (не чиним здесь)

- Дегенеративная рамка: `commit_frame_edit` сегодня снимает chrome до проверки
  «рамка < 1 px» и возвращает режим без OK-кнопки и метки (до переключения
  инструмента). Порядок «убрать chrome → спросить машину → None → перелэйаут»
  сохраняется как есть — это наблюдаемое поведение, чинить его здесь нельзя.
- Re-entrancy field-редактора: `commit_text_editor` берёт слоты до
  `removeFromSuperview`; `discard_text_editor` — с тем же комментарием. Машина
  этих точек не касается: ввод текста остаётся владением окна.
- Порядок «коммит текста → разбор клика» в `handle_mouse_down` и «сброс
  выделения до сигнала `DragWindow`» — воспроизводятся как есть.
- `display_shot` сбрасывает коллаут и рамку, но не `drag` — мышь не может быть
  в тяге при смене снимка; машина сохраняет тот же объём сброса.

## Verification

- Каждый таск: `cargo build`, `cargo clippy`, `cargo test`; затем live: собрать
  release, запустить экземпляр (`MINISNAP_DEBUG_OPEN_EDITOR`), скриншотом/AX
  сверить редактор (инструменты, тяга, коллаут, кроп), спрятать окно; экземпляр
  гасить по PID.
- Таск 5 добавляет юнит-тесты машины: MIN_DRAG_PX-сценарии, `draft_is_real`,
  `frame_rect`/`spot_at`, двухфазный коллаут через interface, клампы рамки.
  Машина тестируется без окна и без CGImage: элементы и размеры — значения.
- Правила «Working with a running app»: экземпляры различать по пути бинарника,
  окна прятать после каждой проверки, в стаке держать только свой экземпляр.
