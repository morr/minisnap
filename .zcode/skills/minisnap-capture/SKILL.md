---
name: minisnap-capture
description: Domain map of minisnap's frozen-capture domain — the ScreenCaptureKit T0 snapshot (src/capture.rs) + fullscreen frozen overlay (src/overlay.rs) with region selection, magnifier loupe, click-to-window capture, pulsing red frame, veils over other displays, Esc cancel. Use for tasks touching capture/overlay code or the frozen-capture spec; not shell/hotkeys, output, or editor.
---

# Домен: заморозка кадра и выделение

По хоткею домен фиксирует кадр дисплея под курсором и список видимых окон в момент T0 (`src/capture.rs`), показывает его полноэкранным оверлеем с UI выделения (`src/overlay.rs`) и отдаёт результат — пиксельный прямоугольник плюс изображение — в callback. Норматив — спека `frozen-capture`; сквозные конвенции (потоки, координаты, CGImage, objc2, контракт T0) — скил `minisnap-architecture`, здесь только их специфика. Не входит: запуск по хоткею и preflight (`app.rs`, `permission.rs`), кроп и сохранение (`output.rs`). Перед правкой сверяйся с кодом по адресу.

## Карта модулей

### `src/capture.rs` — снимок момента T0 (фоновый поток)

| Часть | Ответственность | Ключевое |
|---|---|---|
| выбор дисплея | дисплей под курсором (AppKit) и под точкой (SCKit) | `screen_under_cursor`, `pick_display`, `primary_screen_height` (константа флипа Y) |
| список окон T0 | видимые обычные окна, front-to-back | `visible_windows`, `T0Window { frame, window_id, title }` |
| снимок | блокирующие вызовы ScreenCaptureKit | `shareable_content`, `capture_display_image`, `capture_t0`, `T0Capture { image, windows, display_frame }` |
| `mod geometry` | единственный конвертер координат + хит-тест окон | `point_to_sckit`, `selection_to_pixel_rect`, `window_frame_to_pixel_rect`, `display_point_to_pixel`, `intersect`, `clamp_point`, `topmost_window_at` |

### `src/overlay.rs` — полноэкранный оверлей заморозки

| Часть | Ответственность | Ключевое |
|---|---|---|
| публичный вход | окно+вью на весь процесс, переиспользуются | `Overlay::show`, `Overlay::is_active`, `Overlay::cancel` |
| objc2-классы | окно оверлея, вью событий, вью вуали | `OverlayWindow` (`canBecomeKeyWindow = true`), `OverlayView`, `VeilView` |
| состояние | всё в ivars-структуре вью; слои HUD | `OverlayIvars`, `OverlayState` (`Idle`/`Drag`), `VeilIvars` |
| исходы | внутренний запрос финиша и публичный результат | `FinishRequest`, `OverlayOutcome::Selected { pixel_rect, image }` / `Cancelled` |
| лупа | даунсэмпл кадра до логических пикселей | `FrameSampler`, `make_sampler`, `update_hud` |
| затемнение и рамка | затемнение вокруг выделения, красная рамка с пульсом | `update_dim`, `setup_hud`, `start_border_pulse` |

## Поток данных

Вход: `AppController::capture_ready` (гл. поток, тик в `src/app.rs`) → `Overlay::show(t0: T0Capture, screen_frame: CGRect, scale: f64, on_done)`. `screen_frame` — AppKit-глобальные пункты, `scale` — `backingScaleFactor` экрана.

1. `OverlayView::show`: при `active` — игнор с логом (повторный вход). Иначе данные T0 — в ivars; слои HUD цепляются один раз (`layers_attached`); `frame_layer.contents` = кадр; `make_sampler` — даунсэмпл; на все экраны кроме захваченного вешаются вуали (старые `orderOut`); окно показывается и активируется (детали — инварианты); рамка fade-in, затем пульс. Состояние `Idle`.
2. `Idle`: `mouseMoved` → `clamp_point` в границы дисплея → `update_hud`: лупа у курсора (кроп `sampler.image` 20×20 лог. пикселей, nearest, флип у краёв), полные оси через курсор (двухтонные: белая обводка + чёрное ядро), подпись — координаты курсора.
3. `Drag`: `mouseDown` → `Drag`, пульс снят, дырка нулевого размера (полное затемнение); `mouseDragged` → прямоугольник (min/abs от `drag_start`) → `update_dim` (4 чёрные полосы вокруг дырки) + подпись размера; за краем — `clamp_point`; оси скрыты.
4. `mouseUp`: смещение от старта больше `CLICK_TOLERANCE_PT` (3 pt) — drag: `intersect` с экраном → `selection_to_pixel_rect` → `finish(Selected)`. Иначе клик: точка display-local AppKit переводится в SCKit-глобальные вручную (`display_frame_sckit.origin + (x, screen_h − y)`); `topmost_window_at(frames)` → `window_frame_to_pixel_rect` (окно целиком, кламп к кадру); мимо окон — весь кадр `(0, 0, w_px, h_px)`.
5. `finish`: идемпотентен (`active` + `finishing`); вуали `orderOut`, HUD скрыт, `sampler = None`; `FinishRequest::Selected` + кадр из ivars → `OverlayOutcome::Selected { pixel_rect, image }` (кадра нет — `Cancelled`); окно `orderOut` в completion fade-out рамки с guard от ре-входа (короткий флэш заморозки); `on_done(outcome)` — один раз.
6. Отмена → `Cancelled`: Esc (`keyDown`, keyCode 53), клик по вуали (`VeilView::mouseDown` → `on_click`), смена конфигурации дисплеев — два наблюдателя: `OverlayView::screenDidChange:` (NSWorkspace-нотификация) и `AppController::screenParamsChanged:` в `src/app.rs` → `Overlay::cancel()`.

Исход уходит в `src/app.rs`: `Selected` → `output::crop` → буфер/файл/редактор.

## Инварианты и грабли

- Пиксельный размер кадра — только из `CGDisplayMode::pixel_width/height`; `SCDisplay.width` отдаёт пункты. `capture_display_image` задаёт размеры конфига из display mode — иначе Retina-снимок получится в пунктах.
- Список окон T0: `layer == 0 && isOnScreen && pid != свой` (`visible_windows`); порядок SCKit front-to-back сохраняется, `topmost_window_at` берёт первое попадание. Хит-тест клика идёт в SCKit-пространстве: фреймы окон T0 — SCKit-глобальные пункты, клик приходит display-local AppKit, перевод — флип Y относительно `display_frame_sckit`.
- Все прочие переводы — через `capture::geometry` (карта пространств — в `minisnap-architecture`). `pixel_rect` исхода — в пикселях изображения, начало слева вверху; края округляются inward-safe (floor/ceil) и клампятся к кадру.
- Повторный вход защищён в трёх местах: `start_capture` и `capture_ready` в `src/app.rs` (проверка `is_active`) и сам `OverlayView::show`.
- `FrameSampler`: `ctx`, `buf`, `image` живут вместе — контекст не владеет байтами (они в `Vec`), изображение ссылается на буфер. Лупа читает не T0-кадр, а даунсэмпл (factor = `scale.round()`): показывает логические пиксели UI, а не физические; подпись размеров при drag — тоже логические пиксели, хотя спека говорит просто «в пикселях».
- Обновления dim/HUD — внутри `CATransaction::setDisableActions(true)`: иначе Core Animation анимирует слои и лупа «плывёт» за курсором ~0.25 с. Позиция лупы округляется до целых пунктов — иначе субпиксельное скольжение.
- Клавиатура: borderless окно становится key только из-за override `canBecomeKeyWindow`; активация accessory-приложения — `activateIgnoringOtherApps: true` плюс повторный `makeKeyAndOrderFront` через 0.05 с, иначе Esc молчит до первого клика.
- Оверлей и вуали — на уровне `CGShieldingWindowLevel()` (над строкой меню). Вуали пересоздаются на каждый `show`.
- Мёртвый код с `#[allow(dead_code)]`: `line_brightness_in`, `LIGHT_CONTENT_THRESHOLD` — остались для тестов сэмплера (контент-адаптивную раскраску гайдов заменили константной двухтонной).

## Ссылки

- Спека домена: `openspec/specs/frozen-capture/spec.md`.
- Сквозные конвенции: скил `minisnap-architecture` (`references/conventions.md`).
- Код: `src/capture.rs`, `src/overlay.rs`; вызов — `start_capture` / `capture_ready` в `src/app.rs`.
