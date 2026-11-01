# Design

## Context

Решения пункта 3: двойники `show()` (:1060) и `switch_to_surface()` (:1209)
сводятся к одному методу презентации цели; спаривание поверхность↔экран —
чистая структура `SurfaceMap`, считается один раз при `show()` и живёт всю
сессию; размещение — приватный mod внутри `src/overlay.rs`, AppKit-часть
остаётся у `OverlayView`, чистая логика спаривания отдельно и тестируемо.
Адреса в тексте — по текущему состоянию файла (1666 строк, совпадает с
отчётом обзора). Этот design закрепляет interface и промежуточные состояния
тасков.

## Module

### `mod presentation` — приватный mod внутри `src/overlay.rs`

Чистый (без AppKit-вызовов) словарь презентации; `MainThreadMarker`
употребляет только тонкая обвязка `for_surfaces`. Tests — на синтетических
фреймах, без живых экранов и без битмапов.

```rust
/// The live screen behind a surface: AppKit frame and backing scale.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScreenPair {
    pub(crate) frame: CGRect,  // AppKit global points
    pub(crate) scale: f64,     // backingScaleFactor
}

/// Surface↔screen pairing for one overlay session, built once at show().
#[derive(Debug, Default)]
pub(crate) struct SurfaceMap {
    pairs: Vec<Option<ScreenPair>>,  // по индексу поверхности
}

impl SurfaceMap {
    /// Собирает список живых экранов (кадры + backing scale) и высоту
    /// первичного экрана, спаривание делает from_frames.
    pub(crate) fn for_surfaces(surfaces: &[T0Surface], mtm: MainThreadMarker) -> SurfaceMap;

    /// Чистое спаривание: каждой поверхности — экран с совпавшим целым
    /// фреймом после флипа SCKit→AppKit (допуск FRAME_MATCH_TOLERANCE_PT);
    /// без пары — None.
    fn from_frames(
        display_frames_sckit: &[CGRect],
        screens: &[(CGRect, f64)],
        primary_height_pt: f64,
    ) -> SurfaceMap;

    /// Пара-экран поверхности `index`, если он есть.
    pub(crate) fn screen_of(&self, index: usize) -> Option<ScreenPair>;

    /// Первая поверхность, кроме `current`, чья пара-экран содержит
    /// глобальную AppKit-точку (полуоткрытые границы, как сегодня:
    /// `x >= origin.x && x < origin.x + width`); поверхности без пары
    /// не совпадают никогда. Кандидат переключения для mouseMoved.
    pub(crate) fn switch_candidate(&self, point: CGPoint, current: usize) -> Option<usize>;
}
```

**Исключение текущей цели внутри `switch_candidate` (форма сохранена:
interface).** Сегодня поиск в `handle_mouse_moved` сначала отфильтровывает
текущую цель, потом ищет containment (:994–1006). На зеркальных стойках два
экрана могут иметь один фрейм: поиск «сначала containment, потом фильтр»
вернул бы саму цель и после фильтра потерял бы настоящего соседа-зеркало.
Порядок «фильтр → поиск» переезжает в метод как есть.

**Почему спаривание кешируется законно (форма сохранена: seam).** Список
экранов не может измениться посреди сессии: `screenDidChange:`
(NSWorkspace-нотификация) и `AppController::screenParamsChanged:` отменяют
режим целиком. Поэтому карта, построенная в `show()`, верна до `finish()`;
в `finish()` она сбрасывается в `Default` — симметрично очистке
`surfaces`.

### `OverlayView::present_target` — один путь презентации

```rust
/// Единственный путь «показать поверхность index на её паре-экране»:
/// общий для show() и switch_to_surface().
fn present_target(&self, index: usize, screen: ScreenPair);
```

Тело — общий субсегмент обоих двойников, в порядке сегодняшнего
`switch_to_surface()`:

1. `apply_target(index, screen.frame, screen.scale)` + `ivars.target.set(index)`;
2. сброс в idle-презентацию: `state = Idle`, `update_dim(None)`,
   скрытие HUD-слоёв (новый хелпер `hide_hud`, им же пользуется `finish()` —
   сегодня эти шесть `setHidden(true)` повторены в switch :1233–1238 и
   finish :1528–1533);
3. один `CATransaction::setDisableActions(true)`: фрейм root-слоя,
   `contentsScale`+фрейм+`contents` слоя кадра (SAFETY-комментарии
   сохраняются), `contentsScale` десяти HUD-слоёв (хелпер `hud_layers`),
   `rebuild_border_path`;
4. `make_sampler` по новым кадру/скейлу;
5. `rebuild_veils`;
6. окно: `setFrame_display(screen.frame, false)` + `makeKeyAndOrderFront`;
7. рамка: `removeAllAnimations` → сброс invisible без анимации → fade-in
   (`BORDER_FADE_IN_DUR`) → `start_border_pulse`.

### Тонкие `show` / `switch_to_surface`

- `show(t0, on_done)`: гвард повторного входа → `SurfaceMap::for_surfaces`
  → отказ, если у стартовой цели нет пары (тот же лог «no live screen…»,
  ничего на экран не показано) → приём данных сессии в ivars
  (surfaces/windows/on_done/finishing=false/resolving=None/active=true,
  карта) → однократное цепление слоёв (как сегодня) → `setCollectionBehavior`
  → активация приложения (`activateIgnoringOtherApps` + `activate`) →
  `present_target` → отложенный `makeKeyAndOrderFront` через 0.05 с →
  лог-строка.
- `switch_to_surface(index)`: гварды (`active`/`finishing`, чужой индекс) →
  пара из карты, отказ с логом для None → `present_target` → лог-строка →
  `true`.
- `handle_mouse_moved`: поиск кандидата —
  `surface_map.switch_candidate(mouse, ivars.target.get())`; `matched_screen`
  удаляется целиком (все три вызова покрыты картой).

**Активация приложения — сессионное, не целевое (форма сохранена: seam).**
Танец активации (`activateIgnoringOtherApps` + `activate` + отложенный
повторный `makeKeyAndOrderFront`) остаётся в `show()` — он нужен один раз
за сессию, чтобы Esc работал до первого клика; `present_target` делает
только `setFrame_display` + `makeKeyAndOrderFront`. Из-за этого активация
сдвигается на микросекунды раньше переноса окна (сегодня: setFrame → activate →
makeKey; станет: activate → setFrame → makeKey) — статус key-окна
определяется порядком «активация перед makeKey», он сохранён.

## Отклонения и попутные фиксы

Все — следствие сведения путей, а не расширение решений; каждое отмечено
здесь и проверяется live-прогоном.

- **Фикс (форма сохранена: interface).** `show()` получает сбросы
  `switch_to_surface()`-а: idle-сброс (state, dim, HUD), `rebuild_border_path`,
  `contentsScale` HUD-слоёв. На первом показе всё это но-опы (слои свежие,
  setup_hud уже спрятал HUD, state = Idle), на повторном — чинит четыре
  утечки состояния прошлого сеанса:
  1. `OverlayState::Drag` после Esc посреди протяжки жил до следующей
     сессии — `mouseMoved` молчал (гвард `state != Idle`), лупы не было до
     первого клика;
  2. dim-полосы с дыркой прошлой протяжки были видны в новой сессии до
     первого клика;
  3. путь рамки прошлого дисплея рисовался на новом (показ на дисплее
     другого размера после сеанса на прежнем);
  4. `contentsScale` HUD-слоёв прошлого масштаба мылил лупу/подпись до
     первого переключения цели.
- **`finish()` пользуется `hide_hud` (форма сохранена: interface).** Тела
  скрытий идентичны; dim-полосы и `state` finish по-прежнему не трогает —
  это teardown, его поведение не меняется (утечки 1–2 закрываются на входе
  следующей сессии, в present_target).
- **`surface_map` сбрасывается в `finish()` (форма сохранена: interface).**
  Карта не держит Retained-объектов, сброс — симметрия с очисткой
  `surfaces`, семантики не меняет.

## Seam (промежуточные состояния тасков)

Каждый таск — зелёный билд и живой оверлей.

- Таск 1 (SurfaceMap): карта появляется, ivars-слот, все три вызова
  `matched_screen` переходят на неё, `matched_screen` удаляется; show/switch
  пока сохраняют свои тела двойников, но пару-экран берут из карты.
  Промежуточное состояние: данные едины, поведение ещё раздвоено.
- Таск 2 снимает его: `present_target` собирается из общего субсегмента,
  show/switch худеют до обвязки, попутные фиксы въезжают с общим путём.
- Таск 3 — скиллы.

Поведенчески каждый шаг не меняет режим; риск — в переподключении путей
(таск 2), поэтому live-проверка показа и переключения — в обоих тасках.

## Verification

- Каждый таск: `cargo build`, `cargo clippy`, `cargo test`; затем live:
  release-билд, экземпляр запускается со stderr в файл, захват триггерится
  через трей-меню по PID, сверяется лог (`overlay: frozen …`, при
  двухдисплейной стойке — `overlay: target switched to surface …`), окна
  прячутся после каждого действия, экземпляр гасится по PID (правила
  «Working with a running app»).
- Таск 1 добавляет юнит-тесты `SurfaceMap`: спаривание по целому фрейму,
  одинаковые мониторы по origin, допуск, отсутствие пары, кандидат
  переключения (исключение цели, полуоткрытые границы, порядок фильтр→поиск).
- Таск 2: поведение show/switch сверяется с прежним по логам и живому
  окну; четыре попутных фикса проверяются сценарием «Esc посреди протяжки →
  новый захват» (чистый кадр без затемнения, лупа с первого движения).
