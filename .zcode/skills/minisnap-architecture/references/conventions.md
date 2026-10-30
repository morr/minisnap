# Сквозные конвенции minisnap

Подробности конвенций, общих для двух и более доменов. Карта модулей и поток — в `SKILL.md`.

## Потоки

Весь AppKit — только в главном потоке (`MainThreadMarker` у входов). Единственный фоновый поток — захват `capture_t0` (запускается в `start_capture`, `src/app.rs`), результат возвращается по `mpsc` в тик. Глобальные хоткеи не имеют своих потоков: крейт `global-hotkey` опрашивается `HotkeyRegistry::pending_actions()` в том же тике. objc2-объекты не `Send`/`Sync`: между потоками передаются данные (`MainJob`), а не замыкания над контроллером.

## Системы координат

Три пространства (док-комментарий `capture::geometry` — источник истины):

- **AppKit, глобальные пункты** — начало в левом нижнем углу основного дисплея, Y вверх (`NSScreen.frame`, `NSEvent.mouseLocation`);
- **ScreenCaptureKit, глобальные пункты** — начало в левом верхнем, Y вниз (`SCDisplay.frame`, `SCWindow.frame`); перевод: `y_sckit = primary_height_pt − y_appkit`;
- **Пиксели изображения** — начало в левом верхнем углу захваченного дисплея, Y вниз; пункты умножаются на `backingScaleFactor`.

Все переводы — только через `capture::geometry` (`point_to_sckit`, `selection_to_pixel_rect`, `window_frame_to_pixel_rect`, `display_point_to_pixel`): пиксельная точность кропа зависит от единственного конвертера. Редактор живёт в пиксельном пространстве кадра (верхний левый угол, Y вниз) — том же, что и снятые изображения; `editor::Point`/`Rect` — serde-заменители `CGPoint`/`CGRect`. В `CGContext` ось Y перевёрнута: `render::draw_elements` флипает CTM (translate + scale), а вложенные изображения разворачивает обратно `draw_image_upright`.

## CGImage

Кадр — `Retained<CGImage>` в пикселях дисплея: истинный размер берётся из `CGDisplayMode` (`SCDisplay.width` отдаёт пункты, не пиксели). `CGImage` неизменяем и передаётся по ссылкам; через границу потока — владение `Retained` по `mpsc`. Редактор не мутирует базу: любое состояние — это `render_composite(база + элементы)`, файл снимка после правки перезаписывается атомарно (`output::encode_png_atomic`).

## objc2

Свои классы — `define_class!` с `#[thread_kind = MainThreadOnly]` и ivars-структурой (`AppController`, `Overlay`, `EditorWindow`); ivars оборачиваются в `RefCell`/`Cell`. Completion-хендлеры ScreenCaptureKit — `RcBlock` + `mpsc`: хендлер забирает владение через `Retained::retain` (+1) и отправляет результат, функция блокируется на `recv`. Таргеты пунктов меню — слабые ссылки, поэтому контроллер живёт весь процесс (`std::mem::forget` в `app::run`). У каждого `unsafe` вызова — комментарий `SAFETY:`.

## Контракт T0

В момент нажатия хоткея делается один снимок `T0Capture { image, windows, display_frame }`; всё дальнейшее — оверлей, лупа, вырезание результата — читает только его, что бы ни менялось на экране. Смена конфигурации дисплеев (`screenParamsChanged:`) отменяет режим: замороженный кадр больше не соответствует реальности.
