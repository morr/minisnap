---
name: minisnap-editor
description: Map of the minisnap shot-editor domain (src/editor_window.rs + src/editor/) — the editor window, annotation tools (arrow, rectangle, line, pen, text, callout, blur), selection and hit-testing, composite rendering, edit sessions in tmp with journal-replay undo/redo, crop and resize. Use for tasks about the shot editor window, its tools, element rendering, or edit persistence; not for capture/overlay, output encoding, or the app shell.
---

# Редактор снимков (shot editor)

Окно, в котором снимок аннотируют и правят: один переиспользуемый `NSWindow` — светлая полоса тулбара сверху, под ней карточка с кадром на шахматке; поверх кадра — векторные элементы и рамки кропа/ресайза. Каждая правка — команда, которая сразу перезаписывает PNG снимка и журнал сессии в tmp. Норматив — спека `shot-editor` (вид окна и зоны переноса, инструменты, коллаут, кроп/ресайз, масштаб показа, сессии, undo); здесь карта реализации: модули, потоки кода и грабли. Границы домена: `src/editor_window.rs` + `src/editor/{mod,render,hit_test,session,text}.rs`. Не входит: запуск окна и маршрутизация хоткеев (`app.rs`, домен shell), PNG-кодирование, буфер и листинг папки (`output.rs`, домен output — редактор их только вызывает), захват и конвертер координат (`capture.rs`).

Сквозные конвенции (потоки, координатные пространства, CGImage, objc2) — скил `minisnap-architecture`; здесь только их применение. Скил даёт адреса, не пересказ кода: перед правкой сверяйся с кодом по адресу.

## Карта модулей

| Модуль | Ответственность | Ключевое |
|---|---|---|
| `src/editor_window.rs` | окно и канвас `EditorView` (свой objc2-класс, MainThreadOnly): мышь, клавиатура, тулбар (ряд контролов, ховер, drag панели), карточка изображения, ввод текста, кроп/ресайз, тюнер стрелки | `EditorWindow`, `KeyableWindow`, `ToolbarPanel`, `EditorView`/`EditorIvars`, `Tool`, `Drag`, `WidthSliderView`, `TextTarget`, `PendingCallout`, `FrameEdit`/`FrameSpot`, `callout_anchor`, `arrow_tuner_panel` |
| `src/editor/mod.rs` | документ: элементы и журнал команд; undo/redo — реплей | `Element`, `ElementKind`, `Command`, `Endpoint`, `EditorDoc`, `Point`/`Rect`/`Rgba` |
| `src/editor/render.rs` | рендер композита и элементов, стрелка-клин, пикселизация блюра | `render_composite`, `draw_elements`, `draw_arrow`, `arrow_outline`, `callout_tail`, `ArrowParams`, `pixelate_region`, `bitmap_context` |
| `src/editor/hit_test.rs` | кто оказался под кликом | `Hit`, `top_hit`, `hit_element`, `text_bounds` |
| `src/editor/session.rs` | сессии в tmp, автосейв после каждой команды | `OpenShot`, `MAX_SESSIONS`, `default_root`, `name_density`, `load`/`save`, `enforce_retention` |
| `src/editor/text.rs` | замеры и отрисовка текста (Core Text) | `measure`, `measure_block`, `line_height`, `draw_text`, `FONT_NAME` |

## Поток данных

1. Открытие: `app::open_editor` → `OpenShot::open(path, default_root())`: живая сессия — `original.png` плюс реплей `commands.json`; без сессии файл сам становится оригиналом нового. Затем `EditorWindow::show_shot(shot)` — окно одно, переиспользуется (живёт в `AppController.editor`, `src/app.rs`).
2. Правка: мышь на канвасе → `Drag` (`Draw`/`Move`/`End`/`Frame`) с живым превью; mouse-up коммитит `Command` → `OpenShot::apply` → `persist`: `render_composite(base, elements)` → `output::encode_png_atomic` в файл снимка + перезапись `commands.json` (только применённый префикс журнала).
3. Undo/redo: `EditorDoc::undo/redo` двигают границу `applied` и пересобирают состояние реплеем префикса по нетронутому `original()`. Обратных операций нет — смещение состояния от журнала невозможно.
4. Выделение: клик → `hit_test::top_hit` → `Hit::Body` (перенос элемента, у текста — правка на месте) или `Hit::End` (тяга конца стрелки/линии/коллаута).
5. Ввод текста и коллаут: прозрачный `NSTextField` поверх блока (`TextTarget::New`/`Existing`); коллаут — стрелка-превью (`PendingCallout`, `Callout.width` замораживается в mouse-down) плюс ввод у основания (`callout_anchor`). Механика обеих фаз и клавиатурная маршрутизация — [references/window-mechanics.md](references/window-mechanics.md).
6. Кроп/ресайз: рамка `FrameEdit` с восемью ручками → `Command::Crop` (отрезать кадр) или `Command::Resize` (пересэмплировать); смена размера базы — правкой или undo — вызывает `refit_window`.

## Инварианты и грабли

- Координаты элементов — пиксели кадра (левый верх, Y вниз). Кадр центрирован в `frame_rect` внутри `card_rect`; `pixel_point` переводит клик в пиксели и клампит в кадр, `pixel_to_view`/`view_scale` — обратный маппинг (им же живёт кегль текста). Плотность показа — из маркера `@<n>x` имени файла (`name_density`): кадр показывается в 1/n. Пустоты карточки — шахматка (`checker_color`, кешированный паттерн) в пределах бордера; служебная графика канвы не выходит за карточку, вне карточки канва не рисует — фон даёт `NSWindow.backgroundColor`.
- Команда применяется к `EditorDoc` до записи на диск; ошибка `persist` идёт в stderr, но не откатывает команду — файл может отстать от состояния в окне.
- Пустой ввод при правке существующего текста удаляет элемент (`DeleteElement`); новый элемент после рисования или ввода текста не выделяется — выделяет клик.
- Маркеры выделения, рамка кропа, поле ввода — графика канваса (`EditorView::draw`), в композит не попадают. Копия в буфер — `render_composite`, кадр с элементами без служебного.
- Блюр не запекает пиксели: `pixelate_region` пересчитывается от базового кадра при каждом рендере (мозаика с блоком 10 px); перенос области открывает исходное под ней. У `Blur` нет цвета и толщины — команды перекраски/толщины для него в журнал не пишутся.
- Слайдер толщины при выделенном штриховом элементе правит его живо; события одной тяги сливаются в одну команду (`apply_width`) — один undo на всю тягу. Каждый redraw ставит ручку слайдера на толщину выделенного (`sync_width_slider`).
- Тулбарная полоса для системы — титлбар окна, и macOS 26 перехватывает её клики под перетаскивание окна: окно `setMovable(false)`, слайдер толщины — наследник `NSControl` со своим tracking loop в `mouseDown:`, перенос окна из режима просмотра — `drag_window_with_mouse`. Механика и история граблей — [references/window-mechanics.md](references/window-mechanics.md).
- `Crop` режет кадр и сдвигает все элементы, `Resize` пересэмплирует и масштабирует координаты (толщины штрихов и кегль текста не масштабируются).
- Сессии — `/tmp/.minisnap/<stem имени файла>/`: `original.png` + `commands.json`, ретенция `MAX_SESSIONS` по mtime каталогов. Ключ — только stem: одинаковые stem из разных папок поделят сессию. Очистка tmp — не ошибка: файл становится новым оригиналом, редактор работает дальше.
- Протяжка короче 4 px (`MIN_DRAG_PX`) — это клик: черновик выбрасывается, элемент не создаётся.
- Режим просмотра (`tool = None`) при открытии: протяжка свободного места снимка переносит окно, протяжка от элемента, маркера или ручки рамки всегда тянет их; инструмент включается кликом кнопки, повторный клик возвращает просмотр.
- Тулбар — `NSStackView` с autolayout-размерами у всех arranged-видов и плитками hover/active отдельными именованными подслоями: явные `setFrame`, ховер через `hitTest` и безымянные слои здесь не работают — вся механика и история граблей в [references/window-mechanics.md](references/window-mechanics.md).
- Отладочные переменные окружения: `MINISNAP_ARROW_DEBUG` — панель тюнера стрелки (`arrow_tuner_panel`) над живыми `render::ARROW_PARAMS`; `MINISNAP_DEBUG_OPEN_EDITOR=<path>` — открыть редактор со снимком сразу при старте (хук в `app.rs`, `MainJob::OpenEditor`). Оба — инструменты разработки, не продукт.
- Текст-элемент правится кликом только при активном инструменте Text/Callout; текст коллаута — любым. Ввод с клавиатуры при выделенном элементе редактор не открывает (расхождение со сценарием спеки «Правка текста»).

## Ссылки

- Норматив — спека `openspec/specs/shot-editor/spec.md`.
- Механика: [references/window-mechanics.md](references/window-mechanics.md) (окно, мышь, клавиатура, ввод текста, коллаут, рамка, палитра/слайдер, плитки тулбара), [references/render-and-hit.md](references/render-and-hit.md) (стрелка, блюр, текст, хит-тест), [references/doc-and-sessions.md](references/doc-and-sessions.md) (команды, журнал, сессии).
- Сквозные конвенции и карта всех модулей: скил `minisnap-architecture`; `encode_png_atomic`, `copy_to_pasteboard`, `sorted_shots` — скил `minisnap-output` (`src/output.rs`).
