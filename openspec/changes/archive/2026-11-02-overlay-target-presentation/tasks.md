# Tasks

## 1. SurfaceMap — спаривание один раз за сессию

- [x] 1.1 В `src/overlay.rs` появляется приватный `mod presentation` с
  `ScreenPair` и `SurfaceMap` (`for_surfaces` / `from_frames` / `screen_of` /
  `switch_candidate`) по design; ivars-слот `surface_map`. `show()` строит
  карту и отказывает без пары у стартовой цели; `switch_to_surface` берёт
  пару из карты; `handle_mouse_moved` ищет кандидата через
  `switch_candidate`; `matched_screen` удаляется; `finish()` сбрасывает
  карту. Тела show/switch пока остаются раздвоенными. Юнит-тесты
  `SurfaceMap` на синтетических фреймах. Проверка: `cargo build`,
  `cargo clippy`, `cargo test`; live — показ оверлея, лупа, переключение
  цели (если стойка двухдисплейная), Esc. *(build/clippy/test зелёные,
  122 теста (+7 SurfaceMap); live: экземпляр по PID 98838 запускался, трей-меню
  работает, но SCK у сессионного экземпляра отдаёт пустой список дисплеев
  («no display under cursor») — запись экрана для сессионного билда отозвана
  TCC (тот же барьер, что в пунктах 1–2; путь захвата change'ом не трогается);
  поведенческая live-проверка показа/переключения ждёт ручной проверки)*

## 2. present_target — один путь презентации

- [x] 2.1 В `src/overlay.rs` появляется `OverlayView::present_target(index,
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
  *(build/clippy/test зелёные, 122 теста; overlay.rs 1666→1848 строк, двойники
  сведены; live: экземпляр по PID 99774 запускался, лог чистый; захват через
  трей-меню упирается в TCC-барьер сессионного билда (SCK отдаёт пустой список
  дисплеев, см. таск 1) — поведенческая live-проверка показа/переключения и
  сценария «Esc посреди протяжки» ждёт ручной проверки)*

## 3. Domain-skills pass

- [x] 3.1 Обновить `.zcode/skills/minisnap-capture/SKILL.md` (карта модулей:
  строка «переключение цели» → `SurfaceMap` + `present_target`; инвариант
  «спаривание считается один раз при show и живёт до конца сессии»; поток
  данных — построение карты в show, запрос кандидата в mouseMoved) и
  `.zcode/skills/minisnap-architecture/SKILL.md` (карта модулей: ключевые
  имена overlay). Проверка: текст скиллов соответствует коду. *(оба SKILL.md
  обновлены: в minisnap-capture — строка карты «презентация цели», вход и
  шаги 1–3 потока данных, два инварианта (спаривание SurfaceMap один раз за
  сессию; сброс idle на обоих путях present_target); в minisnap-architecture —
  ключевые имена overlay; сверено с кодом — все упомянутые имена на месте,
  устаревших упоминаний matched_screen нет)*

## Workflow follow-up

- Архивировать change (`/opsx:archive`), обновить GLOSSARY.md (термины
  «презентация цели», «SurfaceMap» — снять пометку «план»).
