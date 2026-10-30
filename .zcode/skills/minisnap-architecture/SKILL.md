---
name: minisnap-architecture
description: Orientation map of the minisnap macOS screenshot utility codebase — module map, end-to-end data flow from hotkey to editor, threading model, and cross-domain conventions (coordinate spaces, CGImage ownership, objc2 patterns, the T0 freeze contract). Use for project-wide orientation, OpenSpec change planning, cross-domain refactors, or any task that does not clearly belong to a single domain skill.
---

# Архитектура minisnap

Карта кодовой базы для ориентации: какие модули есть, как данные текут от хоткея до редактора, какие конвенции сквозные. Глубина по доменам — в доменных скилах; нормативное поведение продукта — в спеках `openspec/specs/`. Скил даёт карту и адреса, а не копию кода: перед правкой сверяйся с кодом по адресу.

## Карта модулей

| Модуль | Ответственность | Ключевое |
|---|---|---|
| `src/main.rs` | входная точка | `app::run()` |
| `src/app.rs` | жизненный цикл, трей-меню, маршрутизация действий, тик-насос событий | `AppController`, `MainJob`, `start_capture`, `capture_ready`, `open_editor` |
| `src/hotkeys.rs` | глобальные хоткеи: регистрация, опрос событий, клавишные соответствия | `HotkeyAction`, `HotkeyRegistry`, `pending_actions`, `render_combo`, `scancode_to_key` |
| `src/permission.rs` | разрешения Screen Recording / Accessibility | `preflight_screen_capture`, `request_screen_capture`, `is_accessibility_trusted` |
| `src/prefs.rs` | окно настроек: три строки-рекордера комбинаций | `PrefsWindow`, `build`, `ROW_CAPTURE`/`ROW_CAPTURE_EDIT`/`ROW_OPEN_LATEST` |
| `src/config.rs` | конфиг TOML (хоткеи, папка, ретенция), миграция легаси-дефолтов | `Config`, `load_or_create`, `save_to`, `migrate_legacy_defaults` |
| `src/capture.rs` | захват момента T0: дисплей под курсором, кадр в пикселях, список окон; единственный конвертер координат | `capture_t0`, `T0Capture`, `T0Window`, `shareable_content`, `capture_display_image`, `mod geometry` |
| `src/overlay.rs` | полноэкранный оверлей заморозки: выделение, лупа, красная рамка, вуаль соседних дисплеев | `Overlay`, `OverlayOutcome`, `OverlayIvars`, `VeilIvars`, `FrameSampler` |
| `src/output.rs` | вывод результата: буфер обмена, тихое сохранение PNG, имена, ретенция, листинг папки | `crop`, `copy_to_pasteboard`, `save_to_file`, `encode_png_atomic`, `latest_shot`, `sorted_shots`, `enforce_retention` |
| `src/editor_window.rs` | окно редактора: тулбар, инструменты, ввод текста, кроп/ресайз | `EditorWindow`, `Tool`, `Drag`, `EditorIvars`, `FrameEdit`, `PendingCallout` |
| `src/editor/mod.rs` | документ редактора: элементы и журнал команд; undo/redo — реплей префикса по чистому оригиналу | `Element`, `ElementKind`, `Command`, `EditorDoc`, `Point`/`Rect`/`Rgba` |
| `src/editor/render.rs` | рендер элементов и композита, стрелка-клин, пикселизация блюра | `render_composite`, `draw_elements`, `pixelate_region` |
| `src/editor/hit_test.rs` | попадания клика по элементам, границы текстовых блоков | `Hit`, `top_hit`, `text_bounds` |
| `src/editor/session.rs` | сессии правок в tmp: оригинал + журнал, автосейв после каждой команды | `OpenShot`, `MAX_SESSIONS`, `default_root` |
| `src/editor/text.rs` | замеры и отрисовка текста (Core Text) | `measure`, `measure_block`, `line_height`, `draw_text` |

## Сквозной поток «хоткей → снимок → редактор»

1. Хоткей или пункт меню → `AppController::start_capture(edit)` (`src/app.rs`): preflight разрешения, `capture::screen_under_cursor`, конвертация позиции мыши `geometry::point_to_sckit`, фоновый `std::thread` → `capture::capture_t0(mouse_sckit)`.
2. Результат приезжает по `mpsc` как `MainJob::CaptureReady`; метод `tick:` (повторяющийся `NSTimer`, 50 мс) перекачивает jobs в главный поток и там же забирает `HotkeyRegistry::pending_actions()` — глобальные хоткеи опрашиваются, их обработчиков в чужих потоках нет.
3. `capture_ready` → `Overlay::show(t0, screen_frame, scale, callback)`: замороженный кадр накрывает дисплей с курсором.
4. Callback оверлея: `OverlayOutcome::Selected { pixel_rect, image }` → `output::crop` → `output::copy_to_pasteboard` → `output::save_to_file`; при `edit` → `MainJob::OpenEditor { path }` через тот же тик.
5. `open_editor` → `session::OpenShot::open(path, default_root())` → единственное переиспользуемое `EditorWindow::show_shot(shot)`.
6. Правки: `OpenShot::apply`/`apply_width`/`undo`/`redo` → `render::render_composite` → `output::encode_png_atomic` (файл снимка) + журнал сессии в tmp.

## Домены и скилы

| Скил | Домен | Модули | Спеки |
|---|---|---|---|
| `minisnap-shell` | оболочка приложения | `app.rs`, `config.rs`, `prefs.rs`, `hotkeys.rs`, `permission.rs` | `tray-app`, `global-hotkeys`, `preferences`, `screen-recording-permission` |
| `minisnap-capture` | заморозка и выделение | `capture.rs`, `overlay.rs` | `frozen-capture` |
| `minisnap-output` | вывод снимков | `output.rs` | `capture-output` |
| `minisnap-editor` | аннотирование | `editor_window.rs`, `src/editor/` | `shot-editor` |
