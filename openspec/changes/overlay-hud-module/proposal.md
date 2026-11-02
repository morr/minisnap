# Proposal

## Why

Хореография HUD-слоёв оверлея (лупа, оси, подпись, dim-полосы, красная рамка
с пульсом, сэмплер кадра) заперта методами objc2-класса `OverlayView`, хотя
это презентация, а не сессия захвата. Из 34 полей `OverlayIvars` 13 — слои
HUD; знание HUD протекает в шов `present_target` (жёсткий список из 10 слоёв
для `contentsScale` — новый HUD-слой требует правки чужого метода), а
математика размещения лупы (флип у краёв, клэмп сэмпла, снап к целым
пунктам) — чистая функция без единого юнит-теста.

## What Changes

- Новый модуль `src/overlay_hud.rs` со `struct Hud`, владеющий всем
  поддеревом слоёв над `frame_layer` («весь хром»): dim-полосы, лупа с
  сеткой/перекрестием/рамкой, оси, подпись, красная рамка с пульсом и
  `FrameSampler`. `root_layer` получает ровно двух детей: кадр и hud.
- Interface `Hud`: `new()` (разовая сборка и стилизация слоёв),
  `attach(&root_layer, scale)`, `present(scale, screen, frame)` (contentsScale
  всех слоёв, путь рамки, пересборка сэмплера, перезапуск fade-in/пульса —
  вбирает HUD-вставки `present_target`), `update(cursor, selection_px)`
  (покадровое обновление), `dim(Option<CGRect>)`, `hide()`,
  `stop_border_pulse()`.
- `FrameSampler` переезжает вместе с семьёй HUD и получает собственный
  конструктор `FrameSampler::new(w_px, h_px, scale, &image)`; отдельным
  модулем не выносится.
- Чистое ядро размещения лупы и подписи — свободная функция `loupe_layout`
  (флип у краёв, клэмп сэмпла, снап к целым пунктам) с юнит-тестами по
  образцу `SurfaceMap`.
- `OverlayIvars` худеет: 13 HUD-полей уходят, взамен одно поле `hud`;
  `present_target` сжимается — HUD-вставки заменяются вызовами `hud.*`.
  Шов `present_target`/`SurfaceMap` не меняется (установлен прошлым
  раундом), наблюдаемое поведение оверлея не меняется.
- Мёртвый код (`line_brightness_in`, `LIGHT_CONTENT_THRESHOLD`) в новый
  модуль не переезжает; дубль объявления `CGBitmapContextCreate` (extern в
  overlay.rs против крейт-импорта в capture.rs) сводится к крейт-импорту.
- Тесты сэмплера переезжают в `overlay_hud.rs`; тесты раскладки лупы —
  новые.

## Capabilities

### New Capabilities

(нет)

### Modified Capabilities

(нет — чистый рефакторинг: наблюдаемое поведение оверлея не меняется,
спека `frozen-capture` не меняется; change помечен `skip_specs: true`)

## Impact

- Код: `src/overlay.rs` (−~600 строк: семья HUD переезжает, `present_target`
  сжимается, ivars худеют 34 → 22 поля) и новый `src/overlay_hud.rs`
  (~650 строк с тестами); `src/main.rs` (+1 `mod`).
- Скиллы: `.zcode/skills/minisnap-capture/SKILL.md` (карта модулей: строка
  `overlay_hud.rs`, семья HUD за `Hud`) и строка `overlay.rs` в
  `.zcode/skills/minisnap-architecture/SKILL.md` — обновляются в том же
  change'е.
- GLOSSARY.md: термин «HUD оверлея» уже внесён с пометкой «план: change
  overlay-hud-module» — пометка снимается при архивации.
- Риск низкий: поведение не меняется, слои и анимации переезжают как есть;
  live-проверка оверлея (лупа, протяжка, рамка) — по итогам тасков, при
  TCC-барьере сессионного билда ждёт ручной проверки.
