# Tasks

## 1. Containment — одна функция для обоих пространств

- [x] 1.1 В `capture::geometry` появляется `frame_contains_point(frame, point)`
  (полуоткрытые границы, нулевой кадр не содержит) с юнит-тестами (внутри,
  на origin, на max-краях, нулевой кадр, отрицательные координаты);
  `pick_display` (src/capture.rs:105–108) и `SurfaceMap::switch_candidate`
  (src/overlay.rs) переходят на неё, локальный `frame_contains` оверлея
  удаляется. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live —
  показ оверлея, переключение цели между дисплеями (если стойка
  двухдисплейная), клик-за-окно. *(build/clippy/test зелёные, 124 теста
  (+2 containment); live: экземпляр по PID 2410 запускался, трей-меню
  работает, preflight «screen recording: granted», но SCK отдаёт пустой
  список дисплеев («no display under cursor») — TCC-барьер сессионного
  билда, тот же, что в change'ах overlay-target-presentation и ранее; путь
  захвата change'ом не трогается (семантика containment сохранена,
  юнит-тесты покрывают) — live-проверка показа/переключения ждёт ручной
  проверки)*

## 2. Точка→пиксели через geometry, `display_point_to_pixel` возвращается в работу

- [x] 2.1 В `capture::geometry` появляется `display_point_to_logical_pixel`
  (через `display_point_to_pixel`, деление на фактор сэмплера) с юнит-тестами
  (1x, 2x, флип Y, factor 1 при scale 2 — даунсэмпл); сэмпл лупы
  (src/overlay.rs:1122–1125) и подпись координат (:1166–1170) переходят на
  неё; флип клика в mouseUp (:1611–1615) переписывается через
  `display_point_to_pixel` (scale 1.0 + оффсет SCKit-origin); `topmost_window_at`
  удаляется вместе со своими тестами и импортом `CGRectContainsPoint` из
  geometry; модульный `#[allow(dead_code)]` на `pub mod geometry` снимается,
  если билд чист. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live —
  лупа у курсора и подпись координат (приоритет: единственные реально
  меняющиеся пути), протяжка, клик-за-окно, Esc. *(build/clippy/test зелёные,
  123 теста (−2 topmost, +1 logical); `#[allow(dead_code)]` снят, билд чист
  — у каждой функции geometry есть продакшн-вызов; арифметика замен
  побитово идентична прежним формулам; live — тот же TCC-барьер сессионного
  билда, подтверждён в таске 1 этим же билдом (SCK пуст до всякого
  оверлея): лупа/подпись/клик ждут ручной проверки)*

## 3. Domain-skills pass

- [ ] 3.1 Обновить `.zcode/skills/minisnap-capture/SKILL.md` (строка geometry
  в карте модулей: `display_point_to_logical_pixel`, `frame_contains_point`,
  без `topmost_window_at`; инвариант координат — ссылка на спеку
  frozen-capture) и `.zcode/skills/minisnap-architecture/SKILL.md` (карта
  модулей, строка capture.rs). Проверка: текст скиллов соответствует коду.

## Workflow follow-up

- Архивировать change (`/opsx:archive`, спека синхронизируется в
  `openspec/specs/frozen-capture/spec.md`), обновить GLOSSARY.md
  (машинерия «план»-терминов уходит — все change'ы обзора доехали; занести
  `capture::geometry` как единственный конвертер координат).
