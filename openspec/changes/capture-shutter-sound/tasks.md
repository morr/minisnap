# Tasks

Примечание: в рабочем дереве есть незакоммиченные правки `src/app.rs` и
`src/prefs.rs` (другая, незавершённая работа) — их не затрагивать и на них
не опираться; правка обработчика `Selected` в 3.1 должна быть точечной.

## 1. Зависимости

- [x] 1.1 Включить feature `NSSound` в `objc2-app-kit` и добавить прямую
  зависимость `objc2-core-foundation` с feature `CFPreferences` в
  `Cargo.toml`; проверка: `cargo check` проходит без новых предупреждений.

## 2. Модуль звука

- [ ] 2.1 Создать `src/sound.rs` (design D1/D3/D4): чистая функция цепочки
  путей системного звука (`Shutter.aif` в CoreAudio.component, затем
  `camera-shutter.aiff` в ScreenCapture.framework), чтение ключа
  `disable-sound` домена `com.apple.screencapture` через
  `CFPreferencesCopyAppValue`, переиспользуемый инстанс `NSSound` в static,
  best-effort `play_shutter()` (неудачи — запись в stderr, без паники).
  Зарегистрировать модуль в `src/main.rs`. Проверка: `cargo test sound` —
  юнит-тесты: пустая цепочка путей даёт тишину без паники; порядок путей —
  как в design D1; чтение `disable-sound` возвращает bool без паники.

## 3. Подключение к завершению захвата

- [ ] 3.1 В `src/app.rs` в обработчике `OverlayOutcome::Selected` вызвать
  `sound::play_shutter()` сразу после успешного `crop` (design D2 — звук
  до буфера и файла, один вызов на снимок); проверка: `cargo build`
  проходит.

## 4. Живая проверка на работающем приложении

- [ ] 4.1 Собрать и перезапустить инстанс основного чекаута (идентификация
  по PID пути бинарника, по правилам AGENTS.md), сделать снимок: в момент
  завершения выделения слышен щелчок затвора; при нескольких дисплеях —
  один звук на снимок.
- [ ] 4.2 Проверить системный выключатель: с
  `defaults write com.apple.screencapture disable-sound -bool true` снимок
  проходит без звука (сохранение и буфер работают); вернуть
  `defaults delete com.apple.screencapture disable-sound`. Отмена захвата
  (Esc) — тишина.

## 5. Документация домена

- [ ] 5.1 Обновить скилл `minisnap-output`
  (`.zcode/skills/minisnap-output/SKILL.md`): модуль `sound.rs` в карте
  модулей, звук затвора в потоке данных; сверить адреса с фактическим кодом.

## Workflow follow-up

- После ревью — `/opsx:archive` (мерж в master и очистка worktree, если
  работа велась в ветке).
