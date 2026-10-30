---
name: minisnap-shell
description: minisnap shell domain map — app lifecycle and tray menu (app.rs), config with hotkey defaults and legacy-defaults migration (config.rs), the preferences hotkey-recorder window (prefs.rs), global hotkey registration and polling (hotkeys.rs), Screen Recording permission handling (permission.rs). Use for tasks about app startup, tray menu, hotkey registration, config persistence and migration, the Preferences window, or permission dialogs.
---

# Домен shell — оболочка minisnap

Резидентная часть приложения: жизненный цикл (агент без Dock-иконки), трей-меню, тик-насос событий, конфигурация с дефолтами и миграцией легаси, окно настроек, глобальные хоткеи, разрешение Screen Recording. НЕ входит: захват и оверлей (`src/capture.rs`, `src/overlay.rs`), вывод снимков (`src/output.rs`), редактор (`src/editor_window.rs`, `src/editor/`).

## Карта модулей

| Модуль | Ответственность | Ключевое |
|---|---|---|
| `src/app.rs` | lifecycle, трей-меню, маршрутизация действий, тик-насос, диалог разрешения, логика рекордера | `run`, `AppController`, `AppIvars`, `MainJob`, `tick:`, `start_capture`, `recorded_key_event`, `finish_recording`, `show_permission_dialog` |
| `src/config.rs` | TOML-конфиг (хоткеи, `save_dir`, `retention`), дефолты, миграция легаси | `Config`, `standard_path`, `load_or_create`, `load_from_migrating`, `save_to`, `migrate_legacy_defaults`, `legacy_capture_hotkey` |
| `src/prefs.rs` | окно настроек: три строки-рекордера, UI в коде (без xib) | `PrefsWindow` (`window`, `capture_button`, `capture_edit_button`, `open_button`), `build`, `ROW_CAPTURE`/`ROW_CAPTURE_EDIT`/`ROW_OPEN_LATEST` |
| `src/hotkeys.rs` | глобальные хоткеи: регистрация, опрос, конверсии клавишных кодов | `HotkeyAction`, `HotkeyRegistry::apply`, `pending_actions`, `key_equivalent_string`, `modifiers_from_flags`, `scancode_to_key`, `render_combo` |
| `src/permission.rs` | Screen Recording (TCC); Accessibility — только для автотестов | `preflight_screen_capture`, `request_screen_capture`, `is_accessibility_trusted` |

## Поток данных

**Запуск.** `main.rs` → `app::run()`: `NSApplicationActivationPolicy::Accessory` → `Config::load_or_create()` → `AppController::new`: трей-иконка (SF Symbol `camera.viewfinder`), меню, таргеты пунктов на контроллер, `apply_menu_hotkey` и `HotkeyRegistry::apply` из одного конфига, подписка на смену дисплеев, локальный key-monitor, запуск тика. Затем `applicationDidFinishLaunching`: `preflight_screen_capture()`; нет разрешения → `request_screen_capture()` (системный промпт; своего диалога при старте нет).

**Тик-насос.** `tick:` — повторяющийся `NSTimer` 50 мс в `NSRunLoopCommonModes`. За срабатывание: 1) drain `mpsc`-канала `MainJob` — `CaptureReady` → `capture_ready` (оверлей), `OpenEditor` → `open_editor`; 2) `pending_actions()` → `HotkeyAction`: `Capture`/`CaptureEdit` → `start_capture(edit)`, `OpenLatest` → `open_latest`. Пункты меню — второй вход в те же действия.

**Запись комбинации.** Клик кнопки → `prefsRowClicked:` (tag = `ROW_*`) → `start_recording(row)`; следующий keydown ловит локальный монитор → `recorded_key_event`: голый Esc (keycode `0x35`) отменяет, иначе `finish_recording` — запись в `Config`, `save_to(standard_path())`, перерегистрация, `apply_menu_hotkey`, `update_prefs_row_titles`.

**Разрешение.** Каждый `start_capture` делает `preflight_screen_capture()`: нет разрешения → `show_permission_dialog` (модальный NSAlert; «Open System Settings» → `open_screen_recording_settings`), захват не запускается. `applicationDidBecomeActive` re-check'ает и обновляет `permission_granted`.

## Инварианты и грабли

- **Слабые ссылки AppKit компенсируются забыванием.** Таргеты `NSMenuItem` — assign, делегат слабый, поэтому `run()` завершается `std::mem::forget(controller)`. `NSStatusItem` удерживается в `AppIvars` (бар держит items слабо). Тикающий `NSTimer` удерживает target — таймер забыт через `std::mem::forget(timer)` в `schedule_tick`.
- **Хоткеи опрашиваются, колбэков нет.** События крейта `global-hotkey` приезжают в его канал; тик дрейнит `pending_actions()` в главном потоке. Событие со старым id (комбинация уже заменена) дропается; обработчиков в чужих потоках нет.
- **Модальный цикл диалога разрешения пере-входит в тик** (тик в common modes стреляет внутри `runModal`) — флаг `permission_dialog_visible` защищает от стека диалогов.
- **Активация окна из меню-экшена «не клеится»** (меню ещё трекает): `open_prefs_window`/`open_editor` ставят one-shot `NSTimer` 0.0 в `NSDefaultRunLoopMode` → `prefsWindowToFront:` / `editorWindowToFront:`.
- **Один источник правды комбинаций — `Config`**: из него регистрация, шорткаты меню (`key_equivalent_string` + `modifier_flags`) и кнопки настроек (`render_combo`) — разъехаться не могут.
- **Перерегистрация = пересоздание.** `HotkeyRegistry::apply` дропает старый `GlobalHotKeyManager` (это unregisters всё) и создаёт новый; занятая другой приложением комбинация логируется, остальные регистрируются.
- **Локальный key-monitor постоянный.** Ставится один раз при старте (`NSEvent::addLocalMonitorForEventsMatchingMask_handler(KeyDown)`), хранится в `AppIvars.monitor`; активен только при `recording != None`, вне записи события проходят насквозь. В записи глотается всё: неизвестный keycode (запись продолжается), зафиксированная клавиша; пустые модификаторы валидны — голая клавиша станет глобальным хоткеем.
- **Миграция легаси одноразовая.** `migrate_legacy_defaults` срабатывает только если в файле не было `capture_edit_hotkey`: capture на старом дефолте ⌘⌥5 (`legacy_capture_hotkey`) сдвигается на ⌘⌥4, capture-edit получает ⌘⌥5; кастомная capture-комбинация не трогается; `load_or_create` один раз перезаписывает файл.
- **Битый конфиг безопасен.** Ошибка парсинга → дефолты в памяти, файл НЕ перезаписывается; нет файла → дефолты + запись; путь — `standard_path()` = `~/Library/Application Support/minisnap/config.toml`.
- **Иконка трея требует конфигурации.** Голый SF Symbol пуст на полупрозрачном меню-баре — нужна `NSImageSymbolConfiguration` (16 pt, semibold) + `setTemplate(true)`.
- **Повторный вход в захват запрещён.** `start_capture` игнорирует вызов при активном оверлее; `capture_ready` дропает приехавший T0 при уже активном режиме. `screenParamsChanged:` (смена дисплеев) отменяет активный оверлей.
- **Окна ленивые и переиспользуемые.** `PrefsWindow` и `EditorWindow` создаются при первом использовании и живут до конца процесса; у prefs-окна `setReleasedWhenClosed(false)` — закрытие прячет, повторное открытие поднимает то же окно. `open_latest` при пустой папке — молчаливый no-op.
- **Accessibility приложению не нужен** — `is_accessibility_trusted` только для автотестов (синтез нажатий); реальное разрешение одно — Screen Recording. **⌘Q пункта Quit — меню-шорткат**, не глобальная комбинация: работает при открытом меню, глобальное поведение остаётся у активного приложения.

## Ссылки

- Спеки: `openspec/specs/tray-app/spec.md`, `openspec/specs/global-hotkeys/spec.md`, `openspec/specs/preferences/spec.md`, `openspec/specs/screen-recording-permission/spec.md`.
- Сквозные конвенции (потоки, координаты, CGImage, objc2, T0) — скил `minisnap-architecture`.
- Скил — карта, не копия кода: перед правкой сверяйся с кодом по адресу.
