---
name: minisnap-shell
description: minisnap shell domain map — app lifecycle, tray menu and instance identity (app.rs), config with legacy-defaults migration (config.rs), the preferences hotkey-recorder window (prefs.rs), global hotkey registration and polling (hotkeys.rs), Screen Recording permission handling (permission.rs). Use for tasks about app startup, tray menu, instance identity, hotkey registration, config persistence, the Preferences window, or permission dialogs.
---

# Домен shell — оболочка minisnap

Резидентная часть приложения: жизненный цикл (агент без Dock-иконки), идентичность экземпляра (метка чекаута), трей-меню, тик-насос событий, конфигурация с дефолтами и миграцией легаси, окно настроек, глобальные хоткеи, разрешение Screen Recording. НЕ входит: захват и оверлей (`src/capture.rs`, `src/overlay.rs`), вывод снимков (`src/output.rs`, `src/sound.rs`), редактор (`src/editor_window.rs`, `src/editor/`). Норматив поведения (состав меню, дефолтные комбинации, сценарии записи и разрешений) — спеки, см. «Ссылки»; здесь карта реализации.

## Карта модулей

| Модуль | Ответственность |
|---|---|
| `src/app.rs` | lifecycle, трей-меню, идентичность экземпляра, маршрутизация действий, тик-насос, диалог разрешения, клей рекордера, диспетчер агентских запросов (`handle_agent`) |
| `src/agent_proto.rs` | типы запросов/ответов agent-канала (serde), идентичность экземпляра; общие с MCP-мостом |
| `src/agent_server.rs` | HTTP agent-сервер на `127.0.0.1`: listener-поток, реестр экземпляров, доставка в главный поток |
| `src/logbuf.rs` | кольцевой буфер журнала: макрос `log!` (все бывшие `eprintln!`), чтение с offset |
| `src/hotkey_recorder.rs` | чистая машина рекордера: правила записи комбинации без AppKit, юнит-тесты |
| `src/config.rs` | TOML-конфиг (хоткеи, `save_dir`, `retention`, `agent-server-*`), дефолты, миграция легаси |
| `src/prefs.rs` | окно настроек: три строки-рекордера, UI в коде (без xib) |
| `src/hotkeys.rs` | глобальные хоткеи: регистрация, опрос, конверсии клавишных кодов |
| `src/permission.rs` | Screen Recording (TCC); Accessibility — только для автотестов |

## Поток данных

**Запуск.** `main.rs` → `app::run()`: `NSApplicationActivationPolicy::Accessory` → `instance_identity()` (метка чекаута + канонический путь бинарника, одной строкой в stderr) → `Config::load_or_create()` (сводка конфига тоже в stderr) → `AppController::new`: трей-иконка (SF Symbol), меню с инертным заголовком-меткой, таргеты пунктов на контроллер, `apply_menu_hotkey` и `HotkeyRegistry::apply` из одного конфига, подписка на смену дисплеев, локальный key-monitor, запуск тика. Затем `applicationDidFinishLaunching`: `preflight_screen_capture()`; нет разрешения → `request_screen_capture()` (системный промпт; своего диалога при старте нет). Одноразовый отложенный `NSTimer` дергает `sound::prewarm` — прогрев звука затвора не тормозит запуск (механизм звука — `src/sound.rs`, домен output).

**Тик-насос.** `tick:` — повторяющийся `NSTimer` в `NSRunLoopCommonModes`. За срабатывание: 1) drain `mpsc`-канала `MainJob` — `CaptureReady` → `capture_ready` (оверлей), `OpenEditor` → `open_editor`, `Agent` → `handle_agent` (ответ уходит по `mpsc` обратно в listener-поток); 2) `pending_actions()` → `HotkeyAction`: `Capture`/`CaptureEdit` → `start_capture(edit)`, `OpenLatest` → `open_latest`. Пункты меню — второй вход в те же действия.

**Agent-канал.** Встроенный HTTP-сервер (`agent_server.rs`) поднимается при старте, если `agent-server-enabled` (дефолт on): порт — `MINISNAP_AGENT_PORT` > `agent-server-port` конфига > `DEFAULT_AGENT_PORT`. Endpoint'ы: `/ping`, `/state` (полный снимок состояния), `/action` (захват, открытие/закрытие окон, программное завершение оверлея выделением — тот же колбэк результата, что у протяжки), `/editor` (инструмент, цвет, толщина, команда журнала, undo/redo, выделение, листинг — через `OpenShot::apply` + `redraw`), `/screenshot` (`composite` — чистый `render_composite`; `window` — `cacheDisplayInRect` contentView'а, работает при перекрытом окне и на другом Space), `/log`. Каждый ответ несёт `AgentIdentity` (pid, exe, checkout-метка, порт) — путаница двух инстансов исключается проверкой ответа, а не осторожностью. Реестр живых экземпляров — по файлу на процесс `~/Library/Application Support/minisnap/instances/<pid>.json`, запись снимается при graceful quit, мёртвые записи вычищает читающий (MCP сверяет pid записи с pid из ответа). MCP-мост — бинарник `minisnap-mcp` (stdio MCP, зарегистрирован в `.zcode/config.json`): инструменты `instances/state/screenshot/action/editor/log`, конвенция «мёртвое приложение — короткая читаемая ошибка, не паника». Норматив — спека `agent-control`.

**Хоткеи.** Регистрация, опрос, рекордер настроек и миграция легаси — [references/hotkeys.md](references/hotkeys.md); норматив — спеки `global-hotkeys` и `preferences`.

**Разрешение.** Каждый `start_capture` делает `preflight_screen_capture()`: нет разрешения → `show_permission_dialog` (модальный NSAlert; «Open System Settings» → `open_screen_recording_settings`), захват не запускается. `applicationDidBecomeActive` re-check'ает и обновляет `permission_granted`.

## Инварианты и грабли

- **Слабые ссылки AppKit компенсируются забыванием.** Таргеты `NSMenuItem` — assign, делегат слабый, поэтому `run()` завершается `std::mem::forget(controller)`. `NSStatusItem` удерживается в `AppIvars` (бар держит items слабо). Тикающий `NSTimer` удерживает target — таймер забыт через `std::mem::forget(timer)` в `schedule_tick`.
- **Модальный цикл диалога разрешения пере-входит в тик** (тик в common modes стреляет внутри `runModal`) — флаг `permission_dialog_visible` защищает от стека диалогов.
- **Активация окна из меню-экшена «не клеится»** (меню ещё трекает): `open_prefs_window`/`open_editor` ставят one-shot `NSTimer` в `NSDefaultRunLoopMode` → `prefsWindowToFront:` / `editorWindowToFront:`.
- **Битый конфиг безопасен.** Ошибка парсинга → дефолты в памяти, файл НЕ перезаписывается; нет файла → дефолты + запись.
- **Занятый порт агента не роняет приложение.** bind не удался → явная строка через `log!`, сервер не поднимается, приложение живёт без канала; тихой подмены инстанса нет, потому что клиент сверяет идентичность ответа. Запрос, на который главный поток не ответил за 5 с, отдаёт клиенту таймаут — listener не зависает.
- **Агентский захват не показывает диалог разрешения.** `agent_capture` проверяет preflight сам и возвращает `rejected` вместо модального NSAlert на экране пользователя.
- **Иконка трея требует конфигурации.** Голый SF Symbol пуст на полупрозрачном меню-баре — нужна `NSImageSymbolConfiguration` + `setTemplate(true)`.
- **Повторный вход в захват запрещён.** `start_capture` игнорирует вызов при активном оверлее; `capture_ready` дропает приехавший T0 при уже активном режиме. `screenParamsChanged:` (смена дисплеев) отменяет активный оверлей.
- **Окна ленивые и переиспользуемые.** `PrefsWindow` и `EditorWindow` создаются при первом использовании и живут до конца процесса; у prefs-окна `setReleasedWhenClosed(false)` — закрытие прячет, повторное открытие поднимает то же окно. `open_latest` при пустой папке — молчаливый no-op.
- **Идентичность экземпляра — путь бинарника.** `checkout_label`: путь через `.worktrees/<branch>/` → имя ветки, иначе `main`; обязана согласовываться с тем, что `ps -axo comm` показывает для того же процесса. Поверхности метки: строка в stderr при старте, инертный заголовок меню трея и тултип иконки («minisnap — <метка>»), заголовок prefs-окна (`Preferences — <метка>`). Экземпляры неотличимы на глаз — метка единственный способ понять, кем зарегистрирован хоткей.
- **Accessibility приложению не нужен** — `is_accessibility_trusted` только для автотестов (синтез нажатий); реальное разрешение одно — Screen Recording. Шорткат Quit — эквивалент пункта меню, не глобальная комбинация: срабатывает при открытом меню, глобальные клавиши остаются у активного приложения.

## Ссылки

- Норматив поведения — спеки: `openspec/specs/tray-app/spec.md` (состав меню, агент без Dock), `openspec/specs/global-hotkeys/spec.md` (дефолтные комбинации, перерегистрация, сохранение), `openspec/specs/preferences/spec.md` (окно и сценарии записи), `openspec/specs/screen-recording-permission/spec.md`, `openspec/specs/agent-control/spec.md` (endpoint'ы, реестр, MCP-инструменты). Скил — карта реализации, не второй источник норматива.
- Сквозные конвенции (потоки, координаты, CGImage, objc2, T0) — скил `minisnap-architecture`.
