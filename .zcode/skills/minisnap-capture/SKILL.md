---
name: minisnap-capture
description: Domain map of minisnap's frozen-capture domain — the ScreenCaptureKit T0 snapshot of every display (src/capture.rs) + fullscreen frozen overlay (src/overlay.rs) with region selection, magnifier loupe, click-to-window capture, pulsing red frame, veils over other displays, capture target following the cursor between displays, Esc cancel. Use for tasks touching capture/overlay code or the frozen-capture spec; not shell/hotkeys, output, or editor.
---

# Домен: заморозка кадра и выделение

По хоткею домен фиксирует кадры всех подключённых дисплеев и список видимых окон в момент T0 (`src/capture.rs`), показывает кадр дисплея под курсором полноэкранным оверлеем с UI выделения (`src/overlay.rs`); пока режим активен, цель захвата следует за курсором — перевод его на другой дисплей переключает оверлей на его замороженный кадр. Результат — пиксельный прямоугольник плюс изображение — уходит в callback. Норматив — спека `frozen-capture`; сквозные конвенции (потоки, координаты, CGImage, objc2, контракт T0) — скил `minisnap-architecture`, здесь только их специфика. Не входит: запуск по хоткею и preflight (`app.rs`, `permission.rs`), кроп и сохранение (`output.rs`). Перед правкой сверяйся с кодом по адресу.

## Карта модулей

### `src/capture.rs` — снимок момента T0 (фоновый поток)

| Часть | Ответственность | Ключевое |
|---|---|---|
| выбор дисплея | целевой дисплей — под точкой (SCKit) | `pick_display`, `primary_screen_height` (константа флипа Y) |
| список окон T0 | видимые обычные окна, front-to-back — из оконного сервера, не из SCK | `cg_visible_windows`, `T0Window { frame, window_id }` |
| снимок | блокирующие вызовы ScreenCaptureKit, все дисплеи подряд в одном потоке | `shareable_content`, `capture_display_image`, `capture_t0`, `T0Capture { target, surfaces, windows }`, `T0Surface { image, display_frame_sckit }` (дисплей с неудавшимся кадром пропускается, целевой — ошибка) |
| `mod geometry` | единственный конвертер координат + хит-тест окон + сопоставление дисплеев с экранами | `point_to_sckit`, `frame_sckit_to_appkit`, `screen_matches_display` / `FRAME_MATCH_TOLERANCE_PT`, `selection_to_pixel_rect`, `window_frame_to_pixel_rect`, `display_point_to_pixel`, `intersect`, `clamp_point`, `topmost_window_at` |

### `src/overlay.rs` — полноэкранный оверлей заморозки

| Часть | Ответственность | Ключевое |
|---|---|---|
| публичный вход | окно+вью на весь процесс, переиспользуются | `Overlay::show`, `Overlay::is_active`, `Overlay::cancel` |
| objc2-классы | окно оверлея, вью событий, вью вуали | `OverlayWindow` (`canBecomeKeyWindow = true`), `OverlayView`, `VeilView` |
| состояние | всё в ivars-структуре вью; слои HUD; набор поверхностей T0 и индекс цели | `OverlayIvars` (`surfaces`, `target`, `resolving`), `OverlayState` (`Idle`/`Drag`), `VeilIvars` |
| исходы | внутренний запрос финиша и публичный результат | `FinishRequest`, `OverlayOutcome::Selected { pixel_rect, image, scale }` / `Cancelled` |
| лупа | даунсэмпл кадра до логических пикселей | `FrameSampler`, `make_sampler`, `update_hud` |
| затемнение и рамка | затемнение вокруг выделения, красная рамка с пульсом | `update_dim`, `setup_hud`, `rebuild_border_path`, `start_border_pulse` |
| переключение цели | оверлей переезжает на другой дисплей за курсором | `matched_screen`, `apply_target`, `switch_to_surface`, `rebuild_veils` |

## Поток данных

Вход: `AppController::capture_ready` (гл. поток, тик в `src/app.rs`) → `Overlay::show(t0: T0Capture, on_done)`. Фрейм и скейл цели show выводит сам: поверхности сопоставляются с `NSScreen::screens` (`matched_screen` — флип SCKit-фрейма + допуск).

1. `OverlayView::show`: при `active` — игнор с логом (повторный вход). Иначе `surfaces`/`target`/`windows` — в ivars; плоские ivars (кадр, фреймы, скейл) заполняются текущей целью (`apply_target`); слои HUD цепляются один раз (`layers_attached`); `frame_layer.contents` = кадр цели; `make_sampler` — даунсэмпл; на все экраны кроме цели вешаются вуали (`rebuild_veils`, старые `orderOut`); окно показывается и активируется (детали — инварианты); рамка fade-in, затем пульс. Состояние `Idle`.
2. `Idle`: `mouseMoved` — сначала дисплей под курсором: `NSEvent::mouseLocation` (глобальные AppKit-пункты; событие всё равно приходит key-окну, где бы курсор ни был) → экран из `NSScreen::screens` → поверхность с парой-экраном, не равная цели → `switch_to_surface`. На своей цели — прежний путь: `clamp_point` в границы дисплея → `update_hud`: лупа у курсора (кроп `sampler.image` 20×20 лог. пикселей, nearest, флип у краёв), полные оси через курсор (двухтонные: белая обводка + чёрное ядро), подпись — координаты курсора.
3. Переключение цели (`switch_to_surface`): защита повторного входа (`active`/`finishing`, чужой индекс) и отказ для поверхности без пары-экрана; `apply_target` (новые кадр/фреймы/скейл); сброс в `Idle` с очисткой HUD и dim; подмена `frame_layer.contents`/`contentsScale` и геометрии root-слоя внутри `CATransaction` с выключенными actions; `contentsScale` всех HUD-слоев под новый экран; перестройка пути рамки (`rebuild_border_path`); пересборка `FrameSampler` и вуалей (`rebuild_veils` — вуали переезжают: новая цель освобождается, прежняя накрывается); перенос окна `setFrame_display` + `makeKeyAndOrderFront`; fade-in рамки и пульс заново.
4. `Drag`: `mouseDown` → `Drag`, пульс снят, дырка нулевого размера (полное затемнение); `mouseDragged` → прямоугольник (min/abs от `drag_start`) → `update_dim` (4 чёрные полосы вокруг дырки) + подпись размера; за краем — `clamp_point` (переключения в протяжке нет — `handle_mouse_moved` в `Drag` выходит рано); оси скрыты.
5. `mouseUp`: смещение от старта больше `CLICK_TOLERANCE_PT` (3 pt) — drag: `intersect` с экраном → `selection_to_pixel_rect` → `finish(Selected)`. Иначе клик: точка display-local AppKit переводится в глобальные верхнего левого угла вручную (`display_frame_sckit.origin + (x, screen_h − y)`); список окон T0 клонируется, кадр клонируется, и разрешение цели уходит в фоновый поток (`std::thread`) — SCK нельзя звать синхронно из mouseUp: завершения SCK не доходят до main, заблокированного ответом. Ответ (пиксельный прямоугольник) приезжает по mpsc-каналу; канал лежит в `ivars.resolving`, тик приложения в `src/app.rs` каждые 50 мс зовёт `Overlay::poll_resolved_click`, который снимает готовый ответ и `finish(Selected)`; до ответа оверлей висит, гварды в `handle_mouse_down/up` глотают события мыши (режим завершается сам). Само разрешение — `resolve_click_target` (инвариант ниже): кандидаты по CG-порядку, победитель — рендер-проверкой, фолбэк — верхнее окно, мимо — весь кадр `(0, 0, w_px, h_px)`. Список окон T0 общий для всех дисплеев (фреймы окон не привязаны к дисплею), клик работает на любой цели. Сброс `resolving` — в `show()` и `finish()`: после отмены или повторного показа хвостовой канал не должен глотать клики нового сеанса.
6. `finish`: идемпотентен (`active` + `finishing`); вуали `orderOut`, HUD скрыт, `sampler = None`, `surfaces` очищены; `FinishRequest::Selected` + кадр и скейл текущей цели из ivars → `OverlayOutcome::Selected { pixel_rect, image, scale }` (кадра нет — `Cancelled`); окно `orderOut` немедленно — до колбэка, чтобы отклик на mouse-up не ждал обработку результата (кроп, звук, буфер, файл идут уже на открытом рабочем столе); `on_done(outcome)` — один раз.
7. Отмена → `Cancelled`: Esc (`keyDown`, keyCode 53), клик по вуали (`VeilView::mouseDown` → `on_click`; при обычном переводе курсора недостижим — `mouseMoved` переключает цель раньше `mouseDown`, остаётся запасным путём для «телепорта» курсора), смена конфигурации дисплеев — два наблюдателя: `OverlayView::screenDidChange:` (NSWorkspace-нотификация) и `AppController::screenParamsChanged:` в `src/app.rs` → `Overlay::cancel()`.

Исход уходит в `src/app.rs`: `Selected` → `output::crop` → буфер/файл (скейл цели — суффикс `@2x` имени)/редактор.

## Инварианты и грабли

- Пиксельный размер кадра — только из `CGDisplayMode::pixel_width/height`; `SCDisplay.width` отдаёт пункты. `capture_display_image` задаёт размеры конфига из display mode — иначе Retina-снимок получится в пунктах.
- Список окон T0 — из `CGWindowListCopyWindowInfo` (`cg_visible_windows`): layer 0, есть владеющий процесс, только on-screen; собственные видимые окна minisnap участвуют наравне с чужими (норматив — спека `frozen-capture`). Фреймы и front-to-back порядок — истина оконного сервера, они совпадают с композитором (согласованы с AX). Список `SCShareableContent` для этого не годится: у окон, менявших размер вне штатного пути (winit/bevy-игра под оконным менеджером вроде loom), SCK держит устаревшие фреймы и фантомные записи; свежесозданные окна отсутствуют в нём минутами; порядок не следует композитору. Идентификаторы — общий `CGWindowID` namespace, поэтому SCK-список (свежий перезапрос в момент клика) остаётся источником `SCWindow`-объектов для рендер-проверки.
- Победитель клика — `resolve_click_target`: кандидаты — содержащие точку окна T0 в CG-порядке (не больше пяти), каждый рендерится отдельно (`SCContentFilter::initWithDesktopIndependentWindow` + `SCScreenshotConfiguration::setIgnoreShadows(true)` + `SCScreenshotManager::captureScreenshotWithFilter`) и патч 7×7 px в точке сверяется с замороженным кадром; первый совпавший — цель. Порог строгий (`PATCH_MATCH_TOLERANCE = 8` по средней разнице каналов): один и тот же контент через оба конвейера отдаёт единицы, а нестрогий порог (< 16) ложно совпадал на тёмных плоских участках — терминал «проходил» за игровую сцену. Никто не подтвердился (анимированный контент меняется между T0 и рендером, или у окна ещё нет записи в SCK) — фолбэк: верхнее содержащее окно по CG-порядку; содержащих нет — весь кадр.
- Координаты клика: фреймы окон T0 — глобальные пункты с началом в левом верхнем углу (пространство CGWindowList, оно же SCKit-глобальное); клик приходит display-local AppKit, перевод — флип Y относительно `display_frame_sckit`; патчи сравниваются в пикселях изображений (масштаб — фактический размер изображения на ширину фрейма, не ivars-скейл).
- Все прочие переводы — через `capture::geometry` (карта пространств — в `minisnap-architecture`). `pixel_rect` исхода — в пикселях изображения, начало слева вверху; края округляются inward-safe (floor/ceil) и клампятся к кадру.
- Поверхность сопоставляется с экраном целым фреймом (origin + size) после флипа SCKit→AppKit по `primary_screen_height`, с допуском `FRAME_MATCH_TOLERANCE_PT` (~1 pt) — по одному размеру два одинаковых монитора неразличимы. Поверхность без пары-экрана не участвует в переключении: её дисплей остаётся под вуалью до конца режима (поведение «дисплей без кадра T0» из спеки).
- Переключение цели — только в `Idle`; в `Drag` прямоугольник упирается в край дисплея. Повторный вход в `switch_to_surface` защищён теми же `active`/`finishing`, что и `show`.
- Кадры всех дисплеев живут в ivars до конца режима (~60 МБ на 5K-дисплей); освобождаются в `finish()` вместе с текущим кадром. Латентность старта растёт на ~100–200 мс за каждый дополнительный дисплей — цена мгновенного переключения.
- Повторный вход защищён в трёх местах: `start_capture` и `capture_ready` в `src/app.rs` (проверка `is_active`) и сам `OverlayView::show`.
- `FrameSampler`: `ctx`, `buf`, `image` живут вместе — контекст не владеет байтами (они в `Vec`), изображение ссылается на буфер. Лупа читает не T0-кадр, а даунсэмпл (factor = `scale.round()`): показывает логические пиксели UI, а не физические; подпись размеров при drag — тоже логические пиксели, хотя спека говорит просто «в пикселях».
- Обновления dim/HUD и подмена кадра T0 в `show` — внутри `CATransaction::setDisableActions(true)`: иначе Core Animation анимирует слои — лупа «плывёт» за курсором ~0.25 с, а `contents` слоя кадра кроссфейдится с кадром прошлого снимка и при повторном показе мелькает им. Позиция лупы округляется до целых пунктов — иначе субпиксельное скольжение.
- Клавиатура: borderless окно становится key только из-за override `canBecomeKeyWindow`; активация accessory-приложения — `activateIgnoringOtherApps: true` плюс повторный `makeKeyAndOrderFront` через 0.05 с, иначе Esc молчит до первого клика.
- Оверлей и вуали — на уровне `CGShieldingWindowLevel()` (над строкой меню). Вуали пересоздаются на каждый `show` и каждое переключение цели (`rebuild_veils`).
- Мёртвый код с `#[allow(dead_code)]`: `line_brightness_in`, `LIGHT_CONTENT_THRESHOLD` — остались для тестов сэмплера (контент-адаптивную раскраску гайдов заменили константной двухтонной).

## Живая проверка и диагностика

Стандартный прогон: запустить инстанс со stderr в файл (`./target/release/minisnap.app/Contents/MacOS/minisnap >/tmp/minisnap_out.log 2>&1 &`, правила идентификации инстансов — AGENTS.md), триггерить захват через трей-меню по PID (osascript `System Events`, у процесса один menu bar с одним айтемом — синтетические глобальные хоткеи не срабатывают), кликать драйвером, читать лог и размеры файлов в `~/Pictures/minisnap` (кроп окна = размер фрейма × 2 на Retina, суффикс `@2x`).

- `scripts/minisnap_drive.swift` — драйвер синтетического ввода: `swiftc -O -o /tmp/minisnap_drive scripts/minisnap_drive.swift`. Режимы `click x,y`, `move|down|drag|up`, `key <kc> <flags>`, `windows` (список окон оконного сервера, layer 0, front-to-back), `screens`, `axcheck`. Координаты — глобальные пункты левого верхнего угла (как в выдаче `windows`).
- `scripts/window_dump.swift` — оба списка окон рядом: `swift scripts/window_dump.swift [cg|sck]`. Этим сравнением и найдено расхождение SCK с реальностью (устаревшие фреймы bevy-окон, фантомные записи) — первый инструмент при «клик взял не то окно».
- Лог в stderr: постоянные однострочные маркеры `click: sckit point …` (клик дошёл до mouseUp), `click: matched window id=…` / `click: no candidate verified, topmost id=…` / `click: no window at the point, whole frame` (исход разрешения), плюс `overlay: …`, `output: saved …`. Отсутствие `click:` при живом клике — события не доходят до вью (см. ниже тайминг).
- `MINISNAP_CLICK_TRACE=1` при запуске — подробный трейс разрешения: кандидаты с фреймами, размеры рендеров, численная разница патчей (`dev=…`), включая `mouse-down` трассировку. Этим ловятся ложные совпадения (высокий `dev` при `matched=true` невозможен — порог виден в константе) и тихие пропуски кандидатов (`not in the SCK list`).
- Грабли тайминга: клик в первые ~0.2 с после появления оверлея может не дойти — mouseDown уходит окну под ещё не вставшим в стек оверлеем, mouseUp игнорируется (нет состояния Drag). В драйверных прогонах ждать строку `overlay: frozen` в логе и ещё ~0.5 с; живому пользователю человеческая реакция даёт запас.

## Ссылки

- Спека домена: `openspec/specs/frozen-capture/spec.md`.
- Сквозные конвенции: скил `minisnap-architecture` (`references/conventions.md`).
- Код: `src/capture.rs`, `src/overlay.rs`; вызов — `start_capture` / `capture_ready` в `src/app.rs`.
- Живые прогоны: `scripts/minisnap_drive.swift`, `scripts/window_dump.swift` (см. «Живая проверка и диагностика»).
