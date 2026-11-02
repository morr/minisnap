# Tasks

## 1. FrameSampler → `src/overlay_hud.rs`

- [x] 1.1 Создать `src/overlay_hud.rs` (`mod overlay_hud;` в `main.rs`):
  структура `FrameSampler` с конструктором
  `FrameSampler::new(w_px, h_px, scale, &image)` (тело бывшего
  `OverlayView::make_sampler`), крейт-импорт `CGBitmapContextCreate` из
  `objc2_core_graphics`; в `overlay.rs` — `use crate::overlay_hud::FrameSampler`,
  ivars-поле `sampler` остаётся, `OverlayView::make_sampler` удаляется,
  extern-блок `CGBitmapContextCreate` (39–49) удаляется вместе с ним,
  `present_target` зовёт `FrameSampler::new`. Тесты сэмплера
  (`synthetic_frame`, `sampler_downsamples_to_logical_pixels`,
  `sampler_handles_scale_one`) переезжают в `overlay_hud.rs`;
  `line_brightness_in` и `LIGHT_CONTENT_THRESHOLD` не переезжают и
  удаляются — тесты читают `buf` напрямую. Проверка: `cargo build`,
  `cargo clippy`, `cargo test`; live — лупа у курсора (если TCC позволит;
  иначе «ждёт ручной проверки»). *(build/clippy/test зелёные: 123 теста,
  clippy-профиль overlay-файлов идентичен базе; конструктор крейта
  возвращает `Option<CFRetained<CGContext>>` — форма вызова сведена к
  capture.rs. Live: экземпляр release-билда PID 99575, «Capture Frozen
  Area» через трей по PID — `overlay: frozen 5120x2880 … surfaces: 2,
  veils: 1`, оверлей жил при движениях мыши, Esc отменил, лог без ошибок;
  визуальная проверка лупы — screencapture не берёт shielding-окно,
  синтетический move ловит SIGTRAP, пользователь активно работал — лупа
  ждёт ручной проверки)*

## 2. `struct Hud` — весь хром за одним interface'ом

- [x] 2.1 В `overlay_hud.rs` появляется `pub(crate) struct Hud`
  (контейнер-слой + dim-полосы + лупа с сеткой/перекрестием/рамкой + оси +
  подпись + рамка с пульсом + сэмплер + `Cell`-копии screen/scale) с
  методами `new`/`attach`/`present`/`update`/`dim`/`hide`/
  `stop_border_pulse`/`drop_sampler` (дизайн-решение 3). В `overlay.rs`:
  13 HUD-полей ivars заменяются полем `hud`; `setup_strip`, `setup_hud`,
  `rebuild_border_path`, `start_border_pulse`, `set_label_text`,
  `update_dim(_inner)`, `update_hud(_inner)`, `hide_hud` переезжают
  телами; HUD-константы (432–468) переезжают; `layers_attached`-блок
  `show` цепляет `hud.attach`; `present_target` сжимается (порядок шагов
  по дизайн-решению 8); `handle_mouse_down/dragged/up/moved` и `finish`
  зовут `hud.*`. Проверка: `cargo build`, `cargo clippy`, `cargo test`;
  live — оверлей целиком: лупа с сеткой и перекрестием у курсора, оси,
  подпись координат, протяжка (dim-полосы + размер), пульс рамки, клик и
  Esc, переключение дисплея при многоэкранной стойке (иначе «ждёт ручной
  проверки»). *(build/clippy/test зелёные: 123 теста, ivars 34 → 22 поля,
  clippy по обоим файлам — только прежнее type_complexity, «for loop over
  a single element» ушёл при переносе; DIM_ALPHA — один источник в
  overlay_hud, overlay.rs импортирует для вуалей. Live: release-билд PID
  10101, «Capture Frozen Area» и «Capture and Edit» через трей по PID —
  оба `overlay: frozen 5120x2880 … surfaces: 2, veils: 1`, повторный
  захват после Esc не «ignored» (сессия закрылась чисто), лог без ошибок;
  визуальная глубина (лупа с сеткой, оси, подпись, протяжка, пульс,
  переключение дисплея) — синтетический move ловит SIGTRAP, screencapture
  не берёт shielding-окно, пользователь активно работал — ждёт ручной
  проверки)*

## 3. Чистое ядро раскладки лупы + тесты

- [x] 3.1 В `overlay_hud.rs` — свободная функция
  `loupe_layout(cursor, screen, scale, sampler) -> LoupeLayout`
  (`loupe_frame`, `label_frame`, `sample_rect`, `cursor_logical`): флип
  лупы у правого/нижнего края, позиция подписи над/под лупой, клэмп сэмпла
  к границам битмапа, снап origin к целым пунктам; `update` пользуется ею,
  арифметика — построчный перенос из `update_hud_inner`. Юнит-тесты по
  образцу `SurfaceMap` (синтетический сэмплер): флип у края, клэмп сэмпла,
  снап к целым пунктам, подпись переворачивается у верхней кромки.
  Проверка: `cargo build`, `cargo clippy`, `cargo test`. *(build/clippy/test
  зелёные: 128 тестов (+5 раскладочных: флипы краёв, снап, клэмп сэмпла,
  переворот подписи, логические пиксели в readout); в структуре ещё
  `cursor_logical` — текст подписи не считает конверсию второй раз;
  арифметика перенесена построчно, вызов `update` в live-пути не менялся)*

## 4. Domain-skills pass

- [x] 4.1 Обновить `.zcode/skills/minisnap-capture/SKILL.md` (карта
  модулей: строка `src/overlay_hud.rs` — семья HUD за `Hud`; строка
  `overlay.rs` без HUD-полей; инварианты: новый HUD-слой не требует правок
  `present_target`, контейнер — второй ребёнок root) и строку `overlay.rs`
  в `.zcode/skills/minisnap-architecture/SKILL.md` (модульная карта).
  Проверка: текст скиллов соответствует коду. *(minisnap-capture:
  описание и вводка называют оба файла; таблица overlay.rs без HUD-строк
  «лупа»/«затемнение и рамка», `OverlayIvars` с полем `hud`; новая
  таблица `src/overlay_hud.rs` — chrome-слои/сэмплер/раскладка; шаги 1–4
  и 6 потока переведены на вызовы `hud.*`; инвариант «HUD — один
  владелец» добавлен, в буллете FrameSampler — судьба `buf` (продакшн не
  читает, обязан жить рядом с image), буллет про `line_brightness_in`
  удалён; минус устаревший буллет про мёртвый код. minisnap-architecture:
  строка overlay.rs без FrameSampler, новая строка overlay_hud.rs.
  Сверено с кодом: все 8 методов `Hud`, `FrameSampler::new`,
  `loupe_layout`/`LoupeLayout`, поле `hud: Hud` и 14 вызовов `hud.*` в
  overlay.rs существуют)*
