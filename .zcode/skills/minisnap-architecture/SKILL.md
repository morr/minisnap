---
name: minisnap-architecture
description: Orientation map of the minisnap macOS screenshot utility codebase — module map, end-to-end data flow from hotkey to editor, threading model, and cross-domain conventions (coordinate spaces, CGImage ownership, objc2 patterns, the T0 freeze contract). Use for project-wide orientation, OpenSpec change planning, cross-domain refactors, or any task that does not clearly belong to a single domain skill.
---

# Архитектура minisnap

Карта кодовой базы для ориентации: какие модули есть, как данные текут от хоткея до редактора, какие конвенции сквозные. Глубина по доменам — в доменных скилах; нормативное поведение продукта — в спеках `openspec/specs/`. Скил даёт карту и адреса, а не копию кода: перед правкой сверяйся с кодом по адресу.

## Карта модулей

| Модуль | Ответственность |
|---|---|
| `src/lib.rs` + `src/main.rs` | библиотека модулей + тонкий бинарник приложения (`app::run()`); второй бинарник — `src/bin/minisnap-mcp.rs` |
| `src/app.rs` | жизненный цикл, трей-меню, идентичность экземпляра, маршрутизация действий; тик-насос — перекачка `MainJob` из `mpsc` и опрос хоткеев в главный поток |
| `src/agent_proto.rs` | типы протокола agent-канала (serde), идентичность экземпляра, общие для приложения и MCP-моста |
| `src/agent_server.rs` | HTTP agent-сервер: listener-поток, разбор запросов, реестр экземпляров, доставка в главный поток через `MainJob::Agent` |
| `src/logbuf.rs` | кольцевой буфер журнала (макрос `log!` вместо `eprintln!`), читается агентом по `/log` |
| `src/hotkeys.rs` | глобальные хоткеи: регистрация, опрос событий, клавишные соответствия сканкодов |
| `src/hotkey_recorder.rs` | чистая машина рекордера комбинаций для настроек: правила записи (повторный вход, Esc-отмена, голая клавиша/неизвестный keycode), без AppKit, юнит-тесты |
| `src/permission.rs` | разрешения Screen Recording / Accessibility |
| `src/prefs.rs` | окно настроек: три строки-рекордера комбинаций |
| `src/config.rs` | конфиг TOML (хоткеи, папка, ретенция), миграция легаси-дефолтов |
| `src/capture.rs` | захват момента T0: кадры всех дисплеев, целевой — под курсором, список окон; единственный конвертер координат (`mod geometry`) |
| `src/overlay.rs` | полноэкранный оверлей заморозки: сессия выделения, вуали соседних дисплеев, цель следует за курсором |
| `src/overlay_hud.rs` | HUD оверлея — весь хром над кадром: dim-полосы, лупа (с даунсэмплом кадра), оси, подпись, красная рамка с пульсом |
| `src/output.rs` | вывод результата: буфер обмена, тихое сохранение PNG, имена, ретенция, листинг папки |
| `src/sound.rs` | звук затвора в момент съёмки: системный файл по цепочке путей, ключ `disable-sound` |
| `src/editor_window.rs` | окно редактора и канвас: ввод текста, chrome рамки; мышиные обработчики тонкие — `pixel_point` + пересылка в машину драга |
| `src/editor_toolbar.rs` | тулбар и хром окна редактора: панель-полоса, слайдер, поповер палитры; стык с ядром — `set_state(ToolbarState)` / `ToolbarEvent` |
| `src/editor_arrow_tuner.rs` | debug-тюнер стрелки за `MINISNAP_ARROW_DEBUG`: панель параметров, спеки, промпт в буфер; события едут через `ToolbarEvent` |
| `src/plates.rs` | общая механика подсвеченных подложек (плит) и hover для тулбара и палитры |
| `src/editor/mod.rs` | документ редактора: элементы и журнал команд; undo/redo — реплей префикса журнала по чистому оригиналу |
| `src/editor/drag.rs` | машина драга: события мыши в пикселях → превью и `Command` в журнал, сигналы наружу (начать текст, тащить окно); двухфазный коллаут и рамка кропа/ресайза; юнит-тесты через interface без окна |
| `src/editor/arrow.rs` | геометрия стрелки-клина: параметризация, силуэт, отрисовка |
| `src/editor/render.rs` | рендер элементов и композита, пикселизация блюра |
| `src/editor/hit_test.rs` | попадания клика по элементам, границы текстовых блоков |
| `src/editor/session.rs` | сессии правок в tmp: оригинал + журнал, автосейв после каждой команды |
| `src/editor/text.rs` | замеры и отрисовка текста (Core Text) |

## Сквозной поток «хоткей → снимок → редактор»

1. Хоткей или пункт меню → `AppController::start_capture(edit)` (`src/app.rs`): preflight разрешения, конвертация позиции мыши `geometry::point_to_sckit`, фоновый `std::thread` → `capture::capture_t0(mouse_sckit)` — снимки всех дисплеев подряд в том же единственном фоновом потоке.
2. Результат приезжает по `mpsc` как `MainJob::CaptureReady`; метод `tick:` (повторяющийся `NSTimer`) перекачивает jobs в главный поток и там же забирает `HotkeyRegistry::pending_actions()` — глобальные хоткеи опрашиваются, их обработчиков в чужих потоках нет.
3. `capture_ready` → `Overlay::show(t0, callback)`: замороженный кадр дисплея с курсором (цель из `T0Capture.target`); пока режим активен, перевод курсора на другой дисплей переключает цель на его кадр.
4. Callback оверлея: `OverlayOutcome::Selected { pixel_rect, image, scale }` → `output::crop` → `sound::play_shutter` → `output::copy_to_pasteboard` → `output::save_to_file`; затем всегда → `MainJob::OpenEditor { path, only_if_visible: !edit }` через тот же тик — «capture and edit» шлёт `only_if_visible: false`, обычный захват `true`, и решение «показывать ли снимок в редакторе» принимает обработчик в `tick:` (см. `minisnap-shell`).
5. `open_editor` → `session::OpenShot::open(path, default_root())` → единственное переиспользуемое `EditorWindow::show_shot(shot)`.
6. Правки: `OpenShot::apply`/`apply_width`/`undo`/`redo` → `render::render_composite` → `output::encode_png_atomic` (файл снимка) + журнал сессии в tmp.

## Сквозные конвенции

Кратко: весь AppKit в главном потоке, единственный фоновый поток — захват T0; три системы координат переводятся только через `capture::geometry` (инвариант нормативно закреплён в спеке `frozen-capture`); `CGImage` неизменяем, редактор перерисовывает композит с нуля; свои objc2-классы — `MainThreadOnly`; заморозка T0 — один набор снимков всех дисплеев, всё дальнейшее читает только его. Подробности — [references/conventions.md](references/conventions.md).

## Домены и скилы

| Скил | Домен | Модули | Спеки |
|---|---|---|---|
| `minisnap-shell` | оболочка приложения | `app.rs`, `config.rs`, `prefs.rs`, `hotkeys.rs`, `hotkey_recorder.rs`, `permission.rs`, `agent_server.rs`, `agent_proto.rs`, `logbuf.rs` | `tray-app`, `global-hotkeys`, `preferences`, `screen-recording-permission`, `agent-control` |
| `minisnap-capture` | заморозка и выделение | `capture.rs`, `overlay.rs` | `frozen-capture` |
| `minisnap-output` | вывод снимков | `output.rs`, `sound.rs` | `capture-output` |
| `minisnap-editor` | аннотирование | `editor_window.rs`, `src/editor/` | `shot-editor` |
