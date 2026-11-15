# Tasks

## 1. Конфигурация и хоткей

- [ ] 1.1 `src/config.rs`: новые поля `upload_base_url` / `upload_token` / `upload_project` / `capture_upload_hotkey` с дефолтами (`https://artifacts.spacehubstore.com`, пусто, `minisnap`, `⌘⌥6`) и TOML-ключами в стиле существующих; юнит-тест: дефолты, сериализация/чтение, частичный конфиг (нет upload-ключей) читается. Проверка: `cargo test`.
- [ ] 1.2 `src/hotkeys.rs`: вариант `HotkeyAction::CaptureUpload`; реестр применяет четвёртую комбинацию из конфига; `src/app.rs` tick-обработчик маппит её в захват с загрузкой. Проверка: `cargo test`.
- [ ] 1.3 `src/app.rs`: `MainJob::CaptureReady { edit: bool }` → `CaptureKind { Plain, Edit, Upload }` (поле `kind` в job, параметр `start_capture`); агентский `Capture { edit }` маппится `true → Edit`, `false → Plain`; ветки существующих действий не меняют поведения. Проверка: `cargo test`.

## 2. Модуль загрузки

- [ ] 2.1 `src/upload.rs` — чистая часть: `upload_name(stem) -> String` (`MiniSnap_…[@<n>x][_N]` → `shot-…`, маркер плотности и локальный суффикс отброшены, правило имени сервера соблюдено), суффиксация `-2…-9`, разбор JSON ответа (`url` из 201/200, ошибка из неуспешных статусов); юнит-тесты на матрицу имён (с маркером, с `@2x_2`, длинное имя) и разбор ответа. Проверка: `cargo test`.
- [ ] 2.2 `src/upload.rs` — транспорт: NSURLSession `uploadTask` (PUT `/api/images/{name}`, `Authorization: Bearer`, `Content-Type: image/png`, `X-Project`, `X-Create-Only: true`), комплишн в фоновой очереди шлёт `MainJob::UploadFinished { result: Result<String, String> }`; 409 → повтор со следующим суффиксом до `-9`; включение фичи `NSURLSession` в objc2-foundation. Проверка: `cargo test` (чистые части), компиляция транспорта; живой прогон — таск 6.
- [ ] 2.3 `src/app.rs`: обработка `UploadFinished` в тике — успех: URL в буфер обмена (хелпер в `src/output.rs` рядом с `copy_to_pasteboard`), уведомить редактор `set_uploading(false)`; ошибка: `NSAlert` с текстом, `set_uploading(false)`. Проверка: `cargo test` (хелпер буфера — компиляция; живой прогон — таск 6).

## 3. Настройки и меню

- [ ] 3.1 `src/prefs.rs`: четвёртый ряд-рекордер «Capture and Upload» (клей через существующий механизм рекордера) и поля токена и проекта; ввод полей сохраняет конфиг сразу (`Config::save_to`); высота окна под пять строк контента. Проверка: `cargo test`; живое открытие окна — таск 6.
- [ ] 3.2 `src/app.rs`: пункт меню «Capture and Upload» между «Capture and Edit» и «Open Latest Image» с серой подсказкой комбинации; `apply_menu_hotkey` обновляет и его. Проверка: `cargo test`.

## 4. Тулбар и редактор

- [ ] 4.1 `src/editor_toolbar.rs`: кнопка Upload в конце ряда (после разделителя за инструментами), `ToolbarEvent::UploadRequested`, поле `uploading` в `ToolbarState` (плитка состояния, повторные клики игнорируются); тесты чистой редукции (`uploading` доходит до состояния кнопки). Проверка: `cargo test`.
- [ ] 4.2 `src/editor_window.rs`: обработка `UploadRequested` → инжектированный при создании колбэк `on_upload(shot_path)`; метод `set_uploading(bool)` пушит состояние через `redraw()`. Проверка: `cargo test`.
- [ ] 4.3 `src/app.rs`: создание `EditorWindow` с колбэком загрузки — пустой токен: диалог «укажите токен в настройках», запрос не отправляется; иначе `upload::upload_file(path, config, jobs_tx)` и `set_uploading(true)`. Проверка: `cargo test`; живой прогон — таск 6.

## 5. Захват с загрузкой

- [ ] 5.1 `src/app.rs`, ветвь `OverlayOutcome::Selected` для `CaptureKind::Upload`: обычный конвейер (кроп, звук, буфер, сохранение) → `upload::upload_file`; `OpenEditor` с `only_if_visible: true` как при обычном захвате. Проверка: `cargo test`; живой прогон — таск 6.

## 6. Живая проверка в запущенном приложении

- [ ] 6.1 Пересобрать бандл (`scripts/build_app.sh release`), перезапустить экземпляр. Кнопка Upload: открыть редактор (снимок из тестового захвата), ввести токен и проект в настройках, клик Upload → `set_uploading` прошёл, буфер содержит публичный URL, изображение открывается по URL (GET), кнопка вернулась в обычное состояние. Проверка: состояния/буфер приложены в recap; окно редактора и настройки скрыты после проверки.
- [ ] 6.2 ⌘⌥6 capture & upload: завершить выделение → файл сохранён, URL в буфере после загрузки; с пустым токеной — диалог-подсказка, запроса нет. Ошибка сервера (неверный токен) → алерт с текстом, приложение живо. Проверка: последовательности `state` и содержимое буфера приложены в recap; окна скрыты.

## 7. Документация

- [ ] 7.1 Скилл `minisnap-output`: добавить `src/upload.rs` в карту модулей (модуль загрузки: транспорт NSURLSession, имена, create-only-повтор) и поток «правка → persist → файл актуален → загрузка». Проверка: строки скилла соответствуют коду.
- [ ] 7.2 Скилл `minisnap-shell`: конфиг (upload-ключи, четвёртый хоткей), prefs (четвёртый ряд, поля токена/проекта), меню (новый пункт), tick (`UploadFinished`). Скилл `minisnap-architecture`: строка `src/upload.rs` в карте модулей. Проверка: строки скиллов соответствуют коду; в recap названы обновлённые разделы.
