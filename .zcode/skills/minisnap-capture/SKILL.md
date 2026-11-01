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
| список окон T0 | видимые обычные окна, front-to-back | `visible_windows`, `T0Window { frame, window_id, title }` |
| снимок | блокирующие вызовы ScreenCaptureKit, все дисплеи подряд в одном потоке | `shareable_content`, `capture_display_image`, `capture_t0`, `T0Capture { target, surfaces, windows }`, `T0Surface { image, display_frame_sckit }` (дисплей с неудавшимся кадром пропускается, целевой — ошибка) |
| `mod geometry` | единственный конвертер координат + хит-тест окон + сопоставление дисплеев с экранами | `point_to_sckit`, `frame_sckit_to_appkit`, `screen_matches_display` / `FRAME_MATCH_TOLERANCE_PT`, `selection_to_pixel_rect`, `window_frame_to_pixel_rect`, `display_point_to_pixel`, `intersect`, `clamp_point`, `topmost_window_at` |

### `src/overlay.rs` — полноэкранный оверлей заморозки

| Часть | Ответственность | Ключевое |
|---|---|---|
| публичный вход | окно+вью на весь процесс, переиспользуются | `Overlay::show`, `Overlay::is_active`, `Overlay::cancel` |
| objc2-классы | окно оверлея, вью событий, вью вуали | `OverlayWindow` (`canBecomeKeyWindow = true`), `OverlayView`, `VeilView` |
| состояние | всё в ivars-структуре вью; слои HUD; набор поверхностей T0 и индекс цели | `OverlayIvars` (`surfaces`, `target`), `OverlayState` (`Idle`/`Drag`), `VeilIvars` |
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
5. `mouseUp`: смещение от старта больше `CLICK_TOLERANCE_PT` (3 pt) — drag: `intersect` с экраном → `selection_to_pixel_rect` → `finish(Selected)`. Иначе клик: точка display-local AppKit переводится в SCKit-глобальные вручную (`display_frame_sckit.origin + (x, screen_h − y)`); `topmost_window_at(frames)` → `window_frame_to_pixel_rect` (окно целиком, кламп к кадру); мимо окон — весь кадр `(0, 0, w_px, h_px)`. Список окон T0 общий для всех дисплеев (SCKit-фреймы окон не привязаны к дисплею), хит-тест работает на любой цели.
6. `finish`: идемпотентен (`active` + `finishing`); вуали `orderOut`, HUD скрыт, `sampler = None`, `surfaces` очищены; `FinishRequest::Selected` + кадр и скейл текущей цели из ivars → `OverlayOutcome::Selected { pixel_rect, image, scale }` (кадра нет — `Cancelled`); окно `orderOut` в completion fade-out рамки с guard от ре-входа (короткий флэш заморозки); `on_done(outcome)` — один раз.
7. Отмена → `Cancelled`: Esc (`keyDown`, keyCode 53), клик по вуали (`VeilView::mouseDown` → `on_click`; при обычном переводе курсора недостижим — `mouseMoved` переключает цель раньше `mouseDown`, остаётся запасным путём для «телепорта» курсора), смена конфигурации дисплеев — два наблюдателя: `OverlayView::screenDidChange:` (NSWorkspace-нотификация) и `AppController::screenParamsChanged:` в `src/app.rs` → `Overlay::cancel()`.

Исход уходит в `src/app.rs`: `Selected` → `output::crop` → буфер/файл (скейл цели — суффикс `@2x` имени)/редактор.

## Инварианты и грабли

- Пиксельный размер кадра — только из `CGDisplayMode::pixel_width/height`; `SCDisplay.width` отдаёт пункты. `capture_display_image` задаёт размеры конфига из display mode — иначе Retina-снимок получится в пунктах.
- Список окон T0: `layer == 0 && isOnScreen` при наличии владеющего приложения (`visible_windows`); собственные видимые окна minisnap лежат в списке наравне с чужими — клик-захват работает и для окна редактора, и для окна настроек. Порядок SCKit front-to-back сохраняется, `topmost_window_at` берёт первое попадание. Хит-тест клика идёт в SCKit-пространстве: фреймы окон T0 — SCKit-глобальные пункты, клик приходит display-local AppKit, перевод — флип Y относительно `display_frame_sckit`.
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

## Ссылки

- Спека домена: `openspec/specs/frozen-capture/spec.md`.
- Сквозные конвенции: скил `minisnap-architecture` (`references/conventions.md`).
- Код: `src/capture.rs`, `src/overlay.rs`; вызов — `start_capture` / `capture_ready` в `src/app.rs`.
