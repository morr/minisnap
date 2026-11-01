---
name: minisnap-shell
description: minisnap shell domain map — app lifecycle, tray menu and instance identity (app.rs), config with legacy-defaults migration (config.rs), the preferences hotkey-recorder window (prefs.rs), global hotkey registration and polling (hotkeys.rs), Screen Recording permission handling (permission.rs). Use for tasks about app startup, tray menu, instance identity, hotkey registration, config persistence, the Preferences window, or permission dialogs.
---

# Домен shell — оболочка minisnap

Резидентная часть приложения: жизненный цикл (агент без Dock-иконки), идентичность экземпляра (метка чекаута), трей-меню, тик-насос событий, конфигурация с дефолтами и миграцией легаси, окно настроек, глобальные хоткеи, разрешение Screen Recording. НЕ входит: захват и оверлей (`src/capture.rs`, `src/overlay.rs`), вывод снимков (`src/output.rs`, `src/sound.rs`), редактор (`src/editor_window.rs`, `src/editor/`). Норматив поведения (состав меню, дефолтные комбинации, сценарии записи и разрешений) — спеки, см. «Ссылки»; здесь карта реализации.

## Карта модулей

| Модуль | Ответственность | Ключевое |
|---|---|---|
| `src/app.rs` | lifecycle, трей-меню, идентичность экземпляра, маршрутизация действий, тик-насос, диалог разрешения, логика рекордера | `run`, `AppController`, `AppIvars`, `MainJob`, `tick:`, `start_capture`, `recorded_key_event`, `finish_recording`, `show_permission_dialog`, `checkout_label`, `instance_identity` |
| `src/config.rs` | TOML-конфиг (хоткеи, `save_dir`, `retention`), дефолты, миграция легаси | `Config`, `standard_path`, `load_or_create`, `load_from_migrating`, `save_to`, `migrate_legacy_defaults`, `legacy_capture_hotkey` |
| `src/prefs.rs` | окно настроек: три строки-рекордера, UI в коде (без xib) | `PrefsWindow` (`window`, `capture_button`, `capture_edit_button`, `open_button`), `build`, `ROW_CAPTURE`/`ROW_CAPTURE_EDIT`/`ROW_OPEN_LATEST` |
| `src/hotkeys.rs` | глобальные хоткеи: регистрация, опрос, конверсии клавишных кодов | `HotkeyAction`, `HotkeyRegistry::apply`, `pending_actions`, `key_equivalent_string`, `modifiers_from_flags`, `scancode_to_key`, `render_combo` |
| `src/permission.rs` | Screen Recording (TCC); Accessibility — только для автотестов | `preflight_screen_capture`, `request_screen_capture`, `is_accessibility_trusted` |

## Поток данных

**Запуск.** `main.rs` → `app::run()`: `NSApplicationActivationPolicy::Accessory` → `instance_identity()` (метка чекаута + канонический путь бинарника, одной строкой в stderr) → `Config::load_or_create()` (сводка конфига тоже в stderr) → `AppController::new`: трей-иконка (SF Symbol `camera.viewfinder`), меню с инертным заголовком-меткой, таргеты пунктов на контроллер, `apply_menu_hotkey` и `HotkeyRegistry::apply` из одного конфига, подписка на смену дисплеев, локальный key-monitor, запуск тика. Затем `applicationDidFinishLaunching`: `preflight_screen_capture()`; нет разрешения → `request_screen_capture()` (системный промпт; своего диалога при старте нет). Одноразовый `NSTimer` через секунду после старта дергает `sound::prewarm` — прогрев звука затвора не тормозит запуск (механизм звука — `src/sound.rs`, домен output).

**Тик-насос.** `tick:` — повторяющийся `NSTimer` 50 мс в `NSRunLoopCommonModes`. За срабатывание: 1) drain `mpsc`-канала `MainJob` — `CaptureReady` → `capture_ready` (оверлей), `OpenEditor` → `open_editor`; 2) `pending_actions()` → `HotkeyAction`: `Capture`/`CaptureEdit` → `start_capture(edit)`, `OpenLatest` → `open_latest`. Пункты меню — второй вход в те же действия.

**Хоткеи.** Регистрация, опрос, рекордер настроек и миграция легаси — [references/hotkeys.md](references/hotkeys.md); норматив — спеки `global-hotkeys` и `preferences`.

**Разрешение.** Каждый `start_capture` делает `preflight_screen_capture()`: нет разрешения → `show_permission_dialog` (модальный NSAlert; «Open System Settings» → `open_screen_recording_settings`), захват не запускается. `applicationDidBecomeActive` re-check'ает и обновляет `permission_granted`.

## Инварианты и грабли

- **Слабые ссылки AppKit компенсируются забыванием.** Таргеты `NSMenuItem` — assign, делегат слабый, поэтому `run()` завершается `std::mem::forget(controller)`. `NSStatusItem` удерживается в `AppIvars` (бар держит items слабо). Тикающий `NSTimer` удерживает target — таймер забыт через `std::mem::forget(timer)` в `schedule_tick`.
- **Модальный цикл диалога разрешения пере-входит в тик** (тик в common modes стреляет внутри `runModal`) — флаг `permission_dialog_visible` защищает от стека диалогов.
- **Активация окна из меню-экшена «не клеится»** (меню ещё трекает): `open_prefs_window`/`open_editor` ставят one-shot `NSTimer` 0.0 в `NSDefaultRunLoopMode` → `prefsWindowToFront:` / `editorWindowToFront:`.
- **Битый конфиг безопасен.** Ошибка парсинга → дефолты в памяти, файл НЕ перезаписывается; нет файла → дефолты + запись; путь — `standard_path()` = `~/Library/Application Support/minisnap/config.toml`.
- **Иконка трея требует конфигурации.** Голый SF Symbol пуст на полупрозрачном меню-баре — нужна `NSImageSymbolConfiguration` (16 pt, semibold) + `setTemplate(true)`.
- **Повторный вход в захват запрещён.** `start_capture` игнорирует вызов при активном оверлее; `capture_ready` дропает приехавший T0 при уже активном режиме. `screenParamsChanged:` (смена дисплеев) отменяет активный оверлей.
- **Окна ленивые и переиспользуемые.** `PrefsWindow` и `EditorWindow` создаются при первом использовании и живут до конца процесса; у prefs-окна `setReleasedWhenClosed(false)` — закрытие прячет, повторное открытие поднимает то же окно. `open_latest` при пустой папке — молчаливый no-op.
- **Идентичность экземпляра — путь бинарника.** `checkout_label`: путь через `.worktrees/<branch>/` → имя ветки, иначе `main`; обязана согласовываться с тем, что `ps -axo comm` показывает для того же процесса. Поверхности метки: строка в stderr при старте, инертный заголовок меню трея и тултип иконки («minisnap — <метка>»), заголовок prefs-окна (`Preferences — <метка>`). Экземпляры неотличимы на глаз — метка единственный способ понять, кем зарегистрирован хоткей.
- **Accessibility приложению не нужен** — `is_accessibility_trusted` только для автотестов (синтез нажатий); реальное разрешение одно — Screen Recording. Шорткат Quit — эквивалент пункта меню, не глобальная комбинация: срабатывает при открытом меню, глобальные клавиши остаются у активного приложения.

## Ссылки

- Норматив поведения — спеки: `openspec/specs/tray-app/spec.md` (состав меню, агент без Dock), `openspec/specs/global-hotkeys/spec.md` (дефолтные комбинации, перерегистрация, сохранение), `openspec/specs/preferences/spec.md` (окно и сценарии записи), `openspec/specs/screen-recording-permission/spec.md`. Скил — карта реализации, не второй источник норматива.
- Сквозные конвенции (потоки, координаты, CGImage, objc2, T0) — скил `minisnap-architecture`.
- Скил — карта, не копия кода: перед правкой сверяйся с кодом по адресу.
