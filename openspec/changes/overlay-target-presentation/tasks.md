# Tasks

## 1. SurfaceMap — спаривание один раз за сессию

- [ ] 1.1 В `src/overlay.rs` появляется приватный `mod presentation` с
  `ScreenPair` и `SurfaceMap` (`for_surfaces` / `from_frames` / `screen_of` /
  `switch_candidate`) по design; ivars-слот `surface_map`. `show()` строит
  карту и отказывает без пары у стартовой цели; `switch_to_surface` берёт
  пару из карты; `handle_mouse_moved` ищет кандидата через
  `switch_candidate`; `matched_screen` удаляется; `finish()` сбрасывает
  карту. Тела show/switch пока остаются раздвоенными. Юнит-тесты
  `SurfaceMap` на синтетических фреймах. Проверка: `cargo build`,
  `cargo clippy`, `cargo test`; live — показ оверлея, лупа, переключение
  цели (если стойка двухдисплейная), Esc.

## 2. present_target — один путь презентации

- [ ] 2.1 В `src/overlay.rs` появляется `OverlayView::present_target(index,
  screen)` — общий субсегмент двойников по design (apply_target, idle-сброс
  через новый `hide_hud`, один CATransaction: геометрия + подмена кадра +
  `contentsScale` HUD + `rebuild_border_path`, sampler, veils, окно, рамка).
  `show()` худеет до приёма сессии + цепления слоёв + активации +
  present_target + отложенного makeKey; `switch_to_surface` — до гвардов +
  present_target; `finish()` пользуется `hide_hud`. Попутные фиксы из design
  (state/dim/border path/HUD contentsScale на повторном показе) въезжают с
  общим путём. Проверка: `cargo build`, `cargo clippy`, `cargo test`; live —
  показ, лупа, протяжка, клик-за-окно, Esc; сценарий «Esc посреди протяжки →
  новый захват» — чистый кадр без затемнения, лупа с первого движения.

## 3. Domain-skills pass

- [ ] 3.1 Обновить `.zcode/skills/minisnap-capture/SKILL.md` (карта модулей:
  строка «переключение цели» → `SurfaceMap` + `present_target`; инвариант
  «спаривание считается один раз при show и живёт до конца сессии»; поток
  данных — построение карты в show, запрос кандидата в mouseMoved) и
  `.zcode/skills/minisnap-architecture/SKILL.md` (карта модулей: ключевые
  имена overlay). Проверка: текст скиллов соответствует коду.

## Workflow follow-up

- Архивировать change (`/opsx:archive`), обновить GLOSSARY.md (термины
  «презентация цели», «SurfaceMap» — снять пометку «план»).
