---
name: minisnap-editor
description: Map of the minisnap shot-editor domain (src/editor_window.rs + src/editor/) — the editor window, annotation tools (arrow, rectangle, line, pen, text, callout, blur), selection and hit-testing, composite rendering, edit sessions in tmp with journal-replay undo/redo, crop and resize. Use for tasks about the shot editor window, its tools, element rendering, or edit persistence; not for capture/overlay, output encoding, or the app shell.
---

# Редактор снимков (shot editor)

Окно, в котором снимок аннотируют и правят: один переиспользуемый `NSWindow` — светлая полоса тулбара сверху, под ней карточка с кадром на шахматке; поверх кадра — векторные элементы и рамки кропа/ресайза. Каждая правка — команда, которая сразу перезаписывает PNG снимка и журнал сессии в tmp. Норматив — спека `shot-editor` (вид окна и зоны переноса, инструменты, коллаут, кроп/ресайз, масштаб показа, сессии, undo); здесь карта реализации: модули, потоки кода и грабли. Границы домена: `src/editor_window.rs` (окно и канва), `src/editor_toolbar.rs` (тулбар и хром), `src/editor_arrow_tuner.rs` (debug-тюнер стрелки), `src/plates.rs` (механика плит) + `src/editor/{mod,arrow,drag,render,hit_test,session,text}.rs`. Не входит: запуск окна и маршрутизация хоткеев (`app.rs`, домен shell), PNG-кодирование, буфер и листинг папки (`output.rs`, домен output — редактор их только вызывает), захват и конвертер координат (`capture.rs`).

Сквозные конвенции (потоки, координатные пространства, CGImage, objc2) — скил `minisnap-architecture`; здесь только их применение.

## Карта модулей

| Модуль | Ответственность |
|---|---|
| `src/editor_window.rs` | окно и канвас `EditorView` (свой objc2-класс, MainThreadOnly): карточка изображения, ввод текста, chrome рамки кропа; обработчики мыши тонкие — `pixel_point` + пересылка события в машину + рисование превью; тулбару собирает `ToolbarState` и отвечает на его события |
| `src/editor_toolbar.rs` | тулбар и хром окна: `ToolbarView` — полоса-панель с собственными ivars (ряд контролов, слайдер, поповер палитры, метка файла, hover, traffic-lights); interface — `set_state(ToolbarState)` на входе, `ToolbarEvent` через один колбэк на выходе (включая варианты `ArrowParam*` — их emit'ит тюнер); чистая редукция `toolbar_state` (документ → снимок) с юнит-тестами без окна |
| `src/editor_arrow_tuner.rs` | debug-тюнер стрелки под `MINISNAP_ARROW_DEBUG`: `ArrowTunerPanel` — свой objc2-вью-владелец, сам держит rows слайдеров, все контролы таргетят панель; наружу — готовые `ToolbarEvent::ArrowParam*` через on-event-колбэк того же вида, что у тулбара |
| `src/plates.rs` | общая механика подсвеченных подложек и hover для двух поверхностей — кнопок тулбара и свотчей палитры: имена слоёв, поиск/центрирование, цвета, кроссфейд |
| `src/editor/mod.rs` | документ: элементы и журнал команд; undo/redo — реплей; одна функция клампов кропа |
| `src/editor/drag.rs` | машина драга — чистый модуль без AppKit-типов: события мыши в пикселях на входе, превью для отрисовки, готовая `Command` и сигналы на выходе; держит три семьи состояния — тянущийся `Drag`, двухфазный коллаут, рамку кропа/ресайза; юнит-тесты через interface без окна |
| `src/editor/arrow.rs` | геометрия стрелки-клина: параметризация, силуэт, отрисовка; единственный глобал домена — живые крутилки стрелки |
| `src/editor/render.rs` | чистый композит: рендер элементов и композита, пикселизация блюра |
| `src/editor/hit_test.rs` | кто оказался под кликом; силуэт стрелки берёт из `arrow.rs`, хвост коллаута — `callout_tail` из `render.rs` |
| `src/editor/session.rs` | сессии в tmp, автосейв после каждой команды |
| `src/editor/text.rs` | замеры и отрисовка текста (Core Text) |

## Поток данных

1. Открытие: `app::open_editor` → `OpenShot::open(path, default_root())`: живая сессия — `original.png` плюс реплей `commands.json`; без сессии файл сам становится оригиналом нового. Затем `EditorWindow::show_shot(shot)` — окно одно, переиспользуется (живёт в `AppController.editor`, `src/app.rs`).
2. Правка: мышь на канвасе → машина драга (`DragMachine` в `editor/drag.rs`): `pixel_point` переводит клик в пиксели, `down`/`drag_to`/`up` ведут `Drag` (`Draw`/`Move`/`End`/`Frame`) с живым превью (`preview`); mouse-up возвращает готовую `Command` → `OpenShot::apply` → `persist`: `render_composite(base, elements)` → `output::encode_png_atomic` в файл снимка + перезапись `commands.json` (только применённый префикс журнала).
3. Undo/redo: `EditorDoc::undo/redo` двигают границу `applied` и пересобирают состояние реплеем префикса по нетронутому `original()`. Обратных операций нет — смещение состояния от журнала невозможно.
4. Выделение: клик → `hit_test::top_hit` → `Hit::Body` (перенос элемента, у текста — правка на месте) или `Hit::End` (тяга конца стрелки/линии/коллаута).
5. Тулбар: канвас собирает `ToolbarState` чистой редукцией из документа и ivars и пушит его из `redraw()` (`ToolbarView::set_state` — единственный вход); пользовательские действия возвращаются как `ToolbarEvent` через один колбэк (замыкание с `Weak<EditorView>`): инструмент — окно применяет правило переключения и рамку, ширина — `apply_width`, пресет — `Recolor`, листание — `page`.
6. Ввод текста и коллаут: прозрачный `NSTextField` поверх блока (`TextTarget::New`/`Existing` — тип живёт в `editor/drag.rs`, слот — в окне); коллаут двухфазный внутри машины — фаза 1: черновик стрелки (`Drag::Draw(Callout)`, ширина заморожена в mouse-down), mouse-up переводит машину в фазу 2 (`PendingCallout` + сигнал `BeginText`), `commit_callout_text` собирает финальный элемент у якоря (`callout_anchor`), пустой ввод сбрасывает фазу. Отрисовка хвоста — `render::callout_tail`; клавиатурная маршрутизация и механика field-редактора — [references/window-mechanics.md](references/window-mechanics.md).
7. Кроп/ресайз: рамка — состояние машины (`begin_frame`/`current_frame`/`commit_frame`) с восемью ручками (допуск в пиксели окну переводит `view_scale`) → `Command::Crop` (отрезать кадр) или `Command::Resize` (пересэмплировать), кламп в кадр — общий `clamp_crop_rect` из `editor/mod.rs`; смена размера базы — правкой или undo — вызывает `refit_window`.

## Инварианты и грабли

- Координаты элементов — пиксели кадра (левый верх, Y вниз). Кадр центрирован в `frame_rect` внутри `card_rect`; `pixel_point` переводит клик в пиксели и клампит в кадр, `pixel_to_view`/`view_scale` — обратный маппинг (им же живёт кегль текста и перевод допуска ручек рамки). Плотность показа — из маркера `@<n>x` имени файла (`name_density`): кадр показывается в 1/n. Пустоты карточки — шахматка (`checker_color`, кешированный паттерн) в пределах бордера; служебная графика канвы не выходит за карточку, вне карточки канва не рисует — фон даёт `NSWindow.backgroundColor`.
- Команда применяется к `EditorDoc` до записи на диск; ошибка `persist` идёт в stderr, но не откатывает команду — файл может отстать от состояния в окне.
- Пустой ввод при правке существующего текста удаляет элемент (`DeleteElement`); новый элемент после рисования или ввода текста не выделяется — выделяет клик.
- Маркеры выделения, рамка кропа, поле ввода — графика канваса (`EditorView::draw`), в композит не попадают. Копия в буфер — `render_composite`, кадр с элементами без служебного.
- Блюр не запекает пиксели: `pixelate_region` пересчитывается от базового кадра при каждом рендере; перенос области открывает исходное под ней. У `Blur` нет цвета и толщины — команды перекраски/толщины для него в журнал не пишутся.
- Слайдер толщины при выделенном штриховом элементе правит его живо; события одной тяги сливаются в одну команду (`apply_width`) — один undo на всю тягу. Пуш состояния из каждого redraw ставит ручку слайдера на толщину выделенного (правило — в редукции `toolbar_state`).
- Тулбарная полоса для системы — титлбар окна, и macOS 26 перехватывает её клики под перетаскивание окна: окно `setMovable(false)`, слайдер толщины — наследник `NSControl` со своим tracking loop в `mouseDown:`, перенос окна из режима просмотра — `drag_window_with_mouse`. Механика и история граблей — [references/window-mechanics.md](references/window-mechanics.md).
- `Crop` режет кадр и сдвигает все элементы, `Resize` пересэмплирует и масштабирует координаты (толщины штрихов и кегль текста не масштабируются).
- Сессии — `/tmp/.minisnap/<stem имени файла>/`: `original.png` + `commands.json`, ретенция `MAX_SESSIONS` по mtime каталогов. Ключ — только stem: одинаковые stem из разных папок поделят сессию. Очистка tmp — не ошибка: файл становится новым оригиналом, редактор работает дальше.
- Протяжка короче `MIN_DRAG_PX` (порог живёт в `editor/drag.rs`) — это клик: черновик выбрасывается, элемент не создаётся. Все правила мышиных сценариев — внутри машины (`DragMachine`), покрыты юнит-тестами через interface без окна; новый инструмент — ветка в `down`/`drag_to`/`up` машины плюс кнопка, не новая логика в обработчиках канвы. Клампы кропа — одна функция `clamp_crop_rect` в `editor/mod.rs`: её зовут и `apply_cmd::Crop`, и `DragMachine::commit_frame`; у рамки поверх клампа свои округление размера и порог дегенеративности (такая рамка остаётся поднятой для новой попытки).
- Режим просмотра (`tool = None`) при открытии: протяжка свободного места снимка переносит окно, протяжка от элемента, маркера или ручки рамки всегда тянет их; инструмент включается кликом кнопки, повторный клик возвращает просмотр.
- Тулбар — свой модуль `editor_toolbar.rs`: канва не держит тулбарных ссылок и говорит с полосой только через `set_state`/`ToolbarEvent`; механика плит общая с палитрой (`plates.rs`). Ряд — `NSStackView` с autolayout-размерами у всех arranged-видов и плитками hover/active отдельными именованными подслоями: явные `setFrame`, ховер через `hitTest` и безымянные слои здесь не работают; полоса обязана фреймиться до добавления ряда (иначе авторезайзинг размазывает стек) — вся механика и история граблей в [references/window-mechanics.md](references/window-mechanics.md).
- Отладочные переменные окружения: `MINISNAP_ARROW_DEBUG` — панель тюнера стрелки (`arrow_tuner_panel` из `src/editor_arrow_tuner.rs`) над живыми `arrow::ARROW_PARAMS`; варианты `ArrowParam*` в `ToolbarEvent` — собственность тюнера: панель emit'ит их через тот же колбэк, окно применяет через `arrow::set_arrow_params`; `MINISNAP_DEBUG_OPEN_EDITOR=<path>` — открыть редактор со снимком сразу при старте (хук в `app.rs`, `MainJob::OpenEditor`). Оба — инструменты разработки, не продукт.
- Текст-элемент правится кликом только при активном инструменте Text/Callout; текст коллаута — любым. Ввод с клавиатуры при выделенном элементе редактор не открывает (расхождение со сценарием спеки «Правка текста»).
- `draw_line` обязан держать text matrix контекста identity (explicit `CGContextSetTextMatrix` перед проходами): `CTLineDraw` домножает её на CTM, а в общем cached-контексте — например, agent-скриншот `cacheDisplayInRect` по всему дереву contentView — AppKit-текст соседнего view (метка файла тулбара) оставляет матрицу перевёрнутой, и глифы канвы выходят вверх ногами (macOS 26). Живой окно-рендер не страдает (свой контекст на view), композит — чистый bitmap; без явной матрицы врал бы только window-скриншот. Проверка: `examples/text_ctx_probe.rs`.

## Ссылки

- Норматив — спека `openspec/specs/shot-editor/spec.md`.
- Механика: [references/window-mechanics.md](references/window-mechanics.md) (окно, мышь, клавиатура, ввод текста, коллаут, рамка, палитра/слайдер, плитки тулбара), [references/render-and-hit.md](references/render-and-hit.md) (стрелка, блюр, текст, хит-тест), [references/doc-and-sessions.md](references/doc-and-sessions.md) (команды, журнал, сессии).
- Сквозные конвенции и карта всех модулей: скил `minisnap-architecture`; `encode_png_atomic`, `copy_to_pasteboard`, `sorted_shots` — скил `minisnap-output` (`src/output.rs`).
