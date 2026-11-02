# Proposal

## Why

Правила записи комбинации в настройках — нетривиальная стейт-машина (защита
от повторного входа, голый Esc отменяет и глотает событие, неизвестный
keycode глотается при продолжающейся записи, пустые модификаторы валидны),
но она сварена с NSEvent и ivars `AppController` в `src/app.rs`
(~L553–662) и не имеет ни одного юнит-теста. Следующий шаг по образцу
машины драга: правила должны стать чистым кодом с табличными тестами.

## What Changes

- Новый чистый модуль `src/hotkey_recorder.rs`: машина рекордера
  `Recorder` — `Recorder::idle() -> Recorder`, `begin(row) -> bool`
  (false = уже пишем другую строку или неизвестная строка),
  `feed(mods, keycode) -> FeedOutcome`, где
  `FeedOutcome = Recorded { row, hotkey } | Cancelled | Swallowed |
  Passthrough`. AppKit-типов в interface нет; зависимости —
  `hotkeys::modifiers_from_flags` не нужны машине (моды приходят готовыми),
  `hotkeys::scancode_to_key`, `HotKey`/`Modifiers` из `global-hotkey`,
  теги `prefs::ROW_*`.
- `src/app.rs` худеет: уходят `start_recording`, логика
  `recorded_key_event` (кроме распаковки NSEvent), `recording: Cell` из
  `AppIvars` (её замещает машина, живущая в `RefCell<Recorder>`).
  Остаются AppKit-клей: селектор `prefsRowClicked:`, установка монитора,
  `finish_recording` (сохранение конфига, перерегистрация хоткеев,
  тайтлы меню) и `update_prefs_row_titles` (тайтлы кнопок).
- Табличные юнит-тесты на все четыре правила (сегодня ноль): повторный
  вход, Esc-отмена с глотанием, неизвестный keycode, пустые модификаторы.
- Попутно: снять протухшие `#[allow(dead_code)]` с реально читаемых полей
  `AppIvars`, если они обнаружатся.
- Наблюдаемое поведение не меняется.

## Capabilities

### New Capabilities

(нет)

### Modified Capabilities

(нет — чистый рефакторинг: правила записи уже нормативны в спеке
`preferences` и не меняются; change помечен `skip_specs: true`)

## Impact

- Код: новый `src/hotkey_recorder.rs` (машина + тесты),
  `src/app.rs` (−~60 строк логики, +тонкий клей), `src/main.rs` (+1 `mod`).
- Скиллы: `.zcode/skills/minisnap-shell/SKILL.md` (карта модулей: строка
  `src/hotkey_recorder.rs`, строка `app.rs` без рекордерной логики) и
  `references/hotkeys.md` — обновляются в том же change'е;
  `.zcode/skills/minisnap-architecture/SKILL.md` — строка нового модуля.
- GLOSSARY.md: термин «Машина рекордера хоткеев» внесён с пометкой
  «план: change hotkey-recorder-machine» — пометка снимается при архивации.
- Риск низкий: правила переезжают построчно, поведение не меняется;
  live-проверка записи комбинации в настройках — по итогам тасков, при
  TCC-барьере сессионного билда ждёт ручной проверки.
