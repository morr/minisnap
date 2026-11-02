# Design

## Context

Рекордер комбинаций в настройках живёт в `src/app.rs` (адреса на момент
планирования): `start_recording` (~L553–565), мониторный колбэк
`recorded_key_event` (~L570–595), `finish_recording` (~L599–627, частично
— AppKit-половина остаётся), `update_prefs_row_titles` (~L631–662,
частично), ivars-поле `recording: Cell<Option<isize>>` (~L83–88).
Аппарат: локальный key-monitor ставится один раз при старте
(~L409–425); селектор `prefsRowClicked:` (~L169–176) получает тег кнопки.
Зависимости логики: `hotkeys::modifiers_from_flags` (перевод
`NSEventModifierFlags` → `global_hotkey::Modifiers` — остаётся в клее,
принимает AppKit-тип), `hotkeys::scancode_to_key` (чистый), константы
`prefs::ROW_*`.

Правила машины: (1) повторный вход в запись запрещён — клик по другой
строке во время записи игнорируется; (2) голый Esc отменяет запись и
глотает событие; (3) неизвестный keycode глотается, запись продолжается;
(4) пустые модификаторы валидны — голая клавиша становится глобальным
хоткеем (`HotKey::new(None, key)`). Сегодня ни одно правило не покрыто
тестом: логика принимает `NonNull<NSEvent>` и пишет в ivars контроллера.

## Goals / Non-Goals

**Goals:**

- Правила записи — чистый код в `src/hotkey_recorder.rs` без AppKit-типов
  в interface, по образцу машины драга (`DragMachine`): состояние внутри,
  наружу — решения.
- Табличные юнит-тесты на все четыре правила.
- `app.rs` остаётся владельцем всего AppKit: распаковка NSEvent (тип
  события, флаги, keycode), монитор, сохранение конфига, перерегистрация
  хоткеев, тайтлы меню и кнопок.
- Наблюдаемое поведение не меняется (включая `eprintln!`-журнал решений).

**Non-Goals:**

- Не менять спеки (`skip_specs`) — правила записи уже нормативны в спеке
  `preferences`.
- Не переносить `modifiers_from_flags` в машину: она принимает
  `NSEventModifierFlags` — AppKit-тип; машина получает готовые
  `Modifiers`.
- Не трогать швы прошлого раунда: машина драга, `present_target`/
  `SurfaceMap`, `capture::geometry`, модули `overlay_hud` и
  `editor_arrow_tuner`, `editor/arrow`.
- Не менять поток событий (монитор → тик не относится; колбэк остаётся
  синхронным в monitor-handler'е).

## Decisions

1. **Interface машины.** `Recorder { recording: Option<isize> }` — тег
   строки (`prefs::ROW_*`), не enum: тег приезжает из `NSButton.tag`
   через селектор, и изобретать конверсию «тег → enum → тег» ради трёх
   констант — churn без выигрыша; `isize` — не AppKit-тип, граница
   «без AppKit в interface» соблюдена.
   - `Recorder::idle() -> Recorder` — стартовое состояние.
   - `begin(&mut self, row: isize) -> bool` — true, запись начата; false,
     если строка неизвестна или запись уже идёт (оба случая в клее —
     молчаливый no-op, как сегодня).
   - `feed(&mut self, mods: Modifiers, keycode: u16) -> FeedOutcome`;
     `enum FeedOutcome { Recorded { row: isize, hotkey: HotKey },
     Cancelled, Swallowed, Passthrough }`. Idle-машина отвечает
     `Passthrough`. Правила построчно из существующего кода: Esc
     (`keycode == 0x35`, mods пусты) → `Cancelled`; `scancode_to_key`
     вернул None → `Swallowed` (запись продолжается); иначе →
     `Recorded { row, hotkey }` с `HotKey::new(if mods.is_empty() {
     None } else { Some(mods) }, key)`. Во всех трёх исходах запись
     завершается, кроме `Swallowed`.
   - `recording_row(&self) -> Option<isize>` — для тайтлов кнопок
     («Press keys…» на записывающейся строке). Это единственный
     добавленный к решениям кандидата доступ — без него тайтлы не
     построить; состояние всё равно одно, дублировать его в клее нельзя.

2. **Клей в `app.rs`.** `AppIvars` вместо `recording: Cell<Option<isize>>`
   получает `recorder: RefCell<hotkey_recorder::Recorder>`. Селектор
   `prefs_row_clicked` вызывает `begin` и при успехе
   `update_prefs_row_titles`. Мониторный колбэк распаковывает NSEvent
   (тип ≠ KeyDown → passthrough — как сегодня), переводит флаги через
   `modifiers_from_flags` и скармливает `feed`; по исходу — `Passthrough`
   → событие проходит, `Swallowed`/`Cancelled`/`Recorded` →
   `std::ptr::null_mut()` (глотание), `Cancelled`/`Recorded` дополнительно
   вызывают `finish_recording` с тем же журналом в stderr.
   `finish_recording` худеет до «применить записанное» (конфиг, save,
   перерегистрация, тайтлы меню) — очистка состояния теперь внутри
   машины. `update_prefs_row_titles` читает строку через
   `recording_row()`.

3. **Граница глотания в клее, не в машине.** Решение «вернуть null vs
   вернуть событие» — про NSEvent, машине оно не нужно: клей мапит
   `Passthrough → event.as_ptr()`, остальное → null. Двусмысленности нет:
   `Swallowed` и `Passthrough` различаются только тем, кто решает.

4. **Тесты — табличные, в модуле** (по образцу тестов `DragMachine` в
   `src/editor/drag.rs`): четыре правила плюс нормальная запись с
   модификаторами и проверка idle→`Passthrough`. `Modifiers`/`Code` —
   из `global_hotkey`, тесты чистые, без окна. Имена кейкодов в тестах —
   через `scancode_to_key` (машина тестируется тем же словарём, которым
   работает).

5. **`#[allow(dead_code)]` в `AppIvars`.** Поля `capture_item`/
   `capture_edit_item`/`open_latest_item` реально читаются
   (`finish_recording` → `apply_menu_hotkey`), их `allow` — протухшие,
   снять. `status_item`/`config` проверяются по факту: если их allow
   тоже протухшие — снять, если поля мертвы — не трогать (не задача
   change'а).

## Risks / Trade-offs

- [Расхождение клея и машины при будущих правках] → машина — единственный
  владелец состояния `recording`; клей не хранит его копии, тайтлы читают
  через `recording_row()`.
- [Тонкий колбэк всё ещё распаковывает событие до проверки idle] →
  порядок «тип события → feed» совпадает с сегодняшним поведением; маска
  монитора — KeyDown-only, расплата — три вызова на keydown, не состояние.
- [Live-проверка записи комбинации требует клавиатуры и TCC] → при
  барьере сессионного билда помечается «ждёт ручной проверки» в tasks.md;
  юнит-тесты машины — страховка правил.

## Migration Plan

Три коммита по tasks.md (модуль с тестами → переподключение app.rs с
снятием протухших allow → domain-skills pass), каждый зелёный по
`cargo build && cargo clippy && cargo test`; четвёртый — архивация с
GLOSSARY.md. Откат — обратный порядок коммитов; публичных API вне крейта
нет.

## Open Questions

(нет)
