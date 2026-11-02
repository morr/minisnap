# Tasks

## 1. Машина рекордера

- [ ] 1.1 Новый `src/hotkey_recorder.rs`: `Recorder` (`idle`, `begin`,
  `feed`, `recording_row`), `FeedOutcome`; `mod` в `main.rs`. Табличные
  юнит-тесты на правила: повторный вход, голый Esc (отмена + глотание),
  неизвестный keycode (глотание при продолжающейся записи), пустые
  модификаторы (голая клавиша), запись с модификаторами, idle →
  `Passthrough`. `cargo build && cargo clippy && cargo test` зелёные;
  модуль пока никем не подключён, поведение не меняется.

## 2. Переподключение app.rs

- [ ] 2.1 `app.rs` на машину: `AppIvars` — `recorder:
  RefCell<Recorder>` вместо `recording: Cell`; `prefs_row_clicked` через
  `begin`; мониторный колбэк — распаковка NSEvent + `feed` + маппинг
  исходов (глотание/пропуск) с прежним stderr-журналом;
  `finish_recording` без очистки состояния (только применение записанного
  и тайтлы); `update_prefs_row_titles` через `recording_row()`. Снять
  протухшие `#[allow(dead_code)]` с реально читаемых полей `AppIvars`.
  `cargo build && cargo clippy && cargo test` зелёные; live-проверка
  записи комбинации в настройках (клик строки → комбинация применяется
  глобально; Esc отменяет) — при TCC-барьере пометить «ждёт ручной
  проверки».

## 3. Скиллы и глоссарий (вместе с архивацией)

- [ ] 3.1 Скилл `minisnap-shell`: карта модулей — строка
  `src/hotkey_recorder.rs`, строка `app.rs` без рекордерной логики;
  `references/hotkeys.md` — механика рекордера через машину.
  `minisnap-architecture`: строка нового модуля в карте. GLOSSARY.md —
  снять пометку «план: change hotkey-recorder-machine» у термина «Машина
  рекордера хоткеев» (указать фактический путь `src/hotkey_recorder.rs`).
