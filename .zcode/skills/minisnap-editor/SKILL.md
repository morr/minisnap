---
name: minisnap-editor
description: Map of the minisnap shot-editor domain (src/editor_window.rs + src/editor/) — the editor window, annotation tools (arrow, rectangle, line, pen, text, callout, blur), selection and hit-testing, composite rendering, edit sessions in tmp with journal-replay undo/redo, crop and resize. Use for tasks about the shot editor window, its tools, element rendering, or edit persistence; not for capture/overlay, output encoding, or the app shell.
---

# Редактор снимков (shot editor)

Окно, в котором снимок аннотируют и правят: один переиспользуемый `NSWindow` показывает кадр целиком на тёмном фоне, поверх — векторные элементы (стрелка, прямоугольник, линия, pen, текст, коллаут, блюр) и рамки кропа/ресайза. Каждая правка — команда, которая сразу перезаписывает PNG снимка и журнал сессии в tmp. Границы домена: `src/editor_window.rs` + `src/editor/{mod,render,hit_test,session,text}.rs`, норматив — спека `shot-editor`. Не входит: запуск окна и маршрутизация хоткеев (`app.rs`, домен shell), PNG-кодирование, буфер и листинг папки (`output.rs`, домен output — редактор их только вызывает), захват и конвертер координат (`capture.rs`).

Сквозные конвенции (потоки, координатные пространства, CGImage, objc2) — скил `minisnap-architecture`; здесь только их применение. Скил даёт адреса, не пересказ кода: перед правкой сверяйся с кодом по адресу.

## Карта модулей

| Модуль | Ответственность | Ключевое |
|---|---|---|
| `src/editor_window.rs` | окно и канвас `EditorView` (свой objc2-класс, MainThreadOnly): мышь, клавиатура, тулбар, ввод текста, кроп/ресайз | `EditorWindow`, `EditorView`/`EditorIvars`, `Tool`, `Drag`, `TextTarget`, `PendingCallout`, `FrameEdit`/`FrameSpot` |
| `src/editor/mod.rs` | документ: элементы и журнал команд; undo/redo — реплей | `Element`, `ElementKind`, `Command`, `Endpoint`, `EditorDoc`, `Point`/`Rect`/`Rgba` |
| `src/editor/render.rs` | рендер композита и элементов, стрелка-клин, пикселизация блюра | `render_composite`, `draw_elements`, `draw_arrow`, `arrow_outline`, `pixelate_region`, `bitmap_context` |
| `src/editor/hit_test.rs` | кто оказался под кликом | `Hit`, `top_hit`, `hit_element`, `text_bounds` |
| `src/editor/session.rs` | сессии в tmp, автосейв после каждой команды | `OpenShot`, `MAX_SESSIONS`, `default_root`, `load`/`save`, `enforce_retention` |
| `src/editor/text.rs` | замеры и отрисовка текста (Core Text) | `measure`, `measure_block`, `line_height`, `draw_text`, `FONT_NAME` |

## Поток данных

1. Открытие: `app::open_editor` → `OpenShot::open(path, default_root())` (корень `/tmp/.minisnap`): живая сессия — `original.png` плюс реплей `commands.json`; без сессии файл сам становится оригиналом нового. Затем `EditorWindow::show_shot(shot)` — окно одно, переиспользуется (живёт в `AppController.editor`, `src/app.rs`).
2. Правка: мышь на канвасе → `Drag` (`Draw`/`Move`/`End`/`Frame`) с живым превью; mouse-up коммитит `Command` → `OpenShot::apply` → `persist`: `render_composite(base, elements)` → `output::encode_png_atomic` в файл снимка + перезапись `commands.json` (только применённый префикс журнала).
3. Undo/redo (⌘Z / ⌘⇧Z): `EditorDoc::undo/redo` двигают границу `applied` и пересобирают состояние реплеем префикса по нетронутому `original()`. Обратных операций нет — смещение состояния от журнала невозможно.
4. Выделение: клик → `hit_test::top_hit` → `Hit::Body` (перенос элемента, у текста — правка на месте) или `Hit::End` (тяга конца стрелки/линии/коллаута).
5. Текст: прозрачный `NSTextField` поверх блока (`TextTarget::New`/`Existing`); Enter — коммит, Shift+Enter — перенос строки, Esc — отмена.
6. Коллаут в две фазы: протяжка стрелки → `PendingCallout` (стрелка висит превью) → ввод текста у основания → `AddElement(Callout)`; пустой текст или отмена — коллаут не создаётся.
7. Кроп/ресайз: рамка `FrameEdit` с восемью ручками → OK/Enter → `Command::Crop` (отрезать кадр) или `Command::Resize` (пересэмплировать); Esc — отмена без команды.

Механика окна и ввода текста — [references/window-mechanics.md](references/window-mechanics.md); стрелка, блюр, текст, хит-тест — [references/render-and-hit.md](references/render-and-hit.md); журнал, команды, сессии — [references/doc-and-sessions.md](references/doc-and-sessions.md).

## Инварианты и грабли

- Координаты элементов — пиксели кадра (левый верх, Y вниз). Канвас вписывает кадр без увеличения; `pixel_point` переводит клик в пиксели и ограничивает его кадром — протяжки не выходят за изображение.
- Команда применяется к `EditorDoc` до записи на диск; ошибка `persist` идёт в stderr, но не откатывает команду — файл может отстать от состояния в окне.
- Новый элемент после рисования или ввода текста НЕ выделяется; выделяет клик. Пустой ввод при правке существующего текста удаляет элемент (`DeleteElement`).
- Маркеры выделения, рамка кропа, поле ввода — графика канваса (`EditorView::draw`), в композит не попадают. ⌘C копирует `render_composite` — кадр с элементами, без служебного.
- Блюр не запекает пиксели: `pixelate_region` пересчитывается от базового кадра при каждом рендере (мозаика с блоком 10 px); перенос области открывает исходное под ней. У `Blur` нет цвета и толщины — команды перекраски/толщины для него в журнал не пишутся.
- Слайдер толщины при выделенном штриховом элементе правит его живо; события одной тяги сливаются в одну команду (`apply_width`) — один undo на всю тягу. Каждый redraw ставит ручку слайдера на толщину выделенного.
- Кроп/ресайз — команды над базой: `Crop` режет кадр и сдвигает все элементы, `Resize` пересэмплирует и масштабирует координаты (толщины штрихов и кегль текста не масштабируются). Смена размера базы — правкой или undo — вызывает `refit_window`.
- Сессия — `/tmp/.minisnap/<stem имени файла>/`: `original.png` + `commands.json`, максимум 50 сессий (ретенция по mtime каталогов). Ключ сессии — только stem. OS очистила tmp — сессии нет, файл становится новым оригиналом, история пуста, редактор работает дальше.
- Протяжка короче 4 px (`MIN_DRAG_PX`) — это клик: черновик выбрасывается, элемент не создаётся.
- Текст-элемент правится кликом только при активном инструменте Text/Callout; текст коллаута — любым. Ввод с клавиатуры при выделенном элементе редактор не открывает (расхождение со сценарием спеки «Правка текста»).

## Ссылки

- Спека домена: `openspec/specs/shot-editor/spec.md`.
- Сквозные конвенции и карта всех модулей: скил `minisnap-architecture`.
- `encode_png_atomic`, `copy_to_pasteboard`, `sorted_shots` — скил `minisnap-output` (`src/output.rs`).
