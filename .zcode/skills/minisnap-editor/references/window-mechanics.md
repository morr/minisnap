# Механика окна редактора (`src/editor_window.rs`)

Детали NSView/NSWindow-механики; конвенции objc2 — в `minisnap-architecture`.

## Устройство

- `EditorWindow { window, view }` — обёртка. Окно создаётся один раз и хранится в `AppController.editor` (`src/app.rs`); `setReleasedWhenClosed(false)` — закрытие только прячет, следующему снимку то же окно. `show_shot` = `display_shot` (заголовок, сброс выделения, `refit_window`) + `makeKeyAndOrderFront`.
- Канвас — свой класс `EditorView: NSView` (`define_class!`, `#[thread_kind = MainThreadOnly]`); состояние — `EditorIvars` в `Cell`/`RefCell`: `shot`, `selection`, `tool`, `color`, `stroke_width`, `drag`, `text_editor` + `text_target`, `frame_edit` + `frame_label`/`frame_ok`, `pending_callout`, `save_dir`, ссылки тулбара (`tool_buttons`, `color_button`, `color_popover`, `width_slider`), `min_width`.
- Раскладка: сверху полоса листания ◄/► (`PAGE_PREV_TAG`/`PAGE_NEXT_TAG`), снизу тулбар (свотч цвета, слайдер толщины, девять кнопок в порядке `Tool::all`), между ними канвас. Кнопки и слайдер `setRefusesFirstResponder(true)`, `initialFirstResponder` — канвас: клавиши никогда не застревают в тулбаре. `min_width` = `toolbar.fittingSize().width + 16` — окно не сжимается ниже тулбара.
- При создании окна активна стрелка (`tool = Some(Tool::Arrow)`); повторный клик по активной кнопке снимает инструмент (`toolClicked:`). После применения рамки кропа/ресайза инструмент сбрасывается в `None`.
- `refit_window`: вписывает кадр в `visibleFrame` главного экрана минус 2 × `TOOLBAR_HEIGHT` (40 pt), без увеличения (`fit_size`, scale ≤ 1), не уже `min_width`, и центрирует окно.
- Иконка коллаута рисуется вручную (`callout_icon`): у SF Symbols нет нужного глифа — текстовые полоски + `draw_arrow` в битмап 30×30.

## Мышь

- `pixel_point` — позиция клика в пикселях кадра (масштаб вида, флип Y), с клампом в кадр.
- mouse-down сперва коммитит открытый текстовый ввод. Дальше по инструменту: Crop/Resize тянут ручки рамки (`frame_spot_at`; кроп при промахе мимо ручек тянет рамку целиком через `FrameSpot::Center`, ресайз не тянет); остальные смотрят `top_hit`: `Hit::End` — тяга конца, `Hit::Body` — правка текста на месте (`hit_edits_text`) или перенос, пустое место — `Drag::Draw` черновика (Text-инструмент вместо черновика сразу открывает ввод; Text поверх чужого элемента — тоже новый текст, `id_is_textlike`).
- Превью во время тяги (`draw`): `Move`/`End` скрывают исходный элемент и рисуют клон с переносом/новым концом; элемент в правке текста скрыт целиком; `Draw` рисует черновик с id 0; `Frame` живёт только в оверлее рамки.
- mouse-up коммитит: `AddElement` / `MoveElement` (только если сдвиг ≥ `MIN_DRAG_PX` — флаг `moved`) / `MovePoint` (при сдвиге ≥ `MIN_DRAG_PX`); для рамки — просто фиксирует rect, команда будет по OK/Enter. `draft_is_real` отбрасывает клики-без-протяжки.

## Клавиатура (`keyDown:` по keyCode)

53 Esc — отменить рамку, иначе закрыть окно; 12+⌘ (⌘Q) — закрыть окно, приложение живёт в трее; 36|76 Enter — применить рамку; 8+⌘ (⌘C) — копия композита; 6+⌘ — undo, +⇧ — redo; 123/124 ←/→ — листание; 51|117 Delete — удалить выделенный.
Листание `page`: коммит открытого текста (правка попадает в файл уходящего снимка), выход из рамки, `output::sorted_shots(save_dir)`, на краях стоп; соседний снимок открывается со своей сессией.

## Ввод текста на месте

- `begin_text_edit(target)`: прозрачный `NSTextField` поверх блока; рамка — `text_editor_frame` по `measure_block` (пустому тексту — фиксированная ширина 200 pt), шрифт `text::FONT_NAME`, кегль в пунктах = размер в пикселях × масштаб вида (минимум 9). Открывается с `selectAll`. Delegate — сам канвас.
- `TextTarget::New { origin, callout, color }` — цвет заморожен в момент клика: смена палитры посреди ввода новый элемент не перекрасит. Новый текст всегда `DEFAULT_TEXT_SIZE_PX` = 32 px.
- Коммит: `insertNewline:` без Shift — коммит (на Enter `controlTextDidEndEditing` в этом окне не срабатывает); клик мимо — коммит через `controlTextDidEndEditing`; Esc — `discard_text_editor`. Shift+Enter пропускается в field editor и вставляет разделитель строк U+2028 — при коммите нормализуется в `\n`.
- Реентерабельность: `removeFromSuperview` синхронно завершает редактирование и входит в `commit_text_editor` повторно — слоты `text_editor`/`text_target` забираются из `RefCell` до снятия поля, повторный вход — no-op. `discard_text_editor` снимает все слоты заранее по той же причине.
- Семантика коммита: пустой текст у `New` — ничего не добавить и выбросить `PendingCallout`; пустой у `Existing` — `DeleteElement`; не изменившийся — команды нет.
- Правка открывается кликом (`hit_edits_text`): блок текста коллаута — любым инструментом; `Text`-элемент — только при активном Text/Callout. Открытия по вводу с клавиатуры нет.

## Коллаут в две фазы

- Тяга рисует стрелку от основания (anchor — точка mouse-down) к острию (tip — курсор): `update_drag` двигает только tip. mouse-up при реальной протяжке кладёт `PendingCallout { base, tip, color, size }` и открывает ввод (`TextTarget::New { callout: true }`); стрелка висит превью до конца ввода.
- Поле ввода ставится у основания (+16 px по x, выше по y); финальный `anchor` на коммите считает свободная функция `callout_anchor(base, text, size, frame)` — блок клампится в кадр с отступом 4 px. Пустой текст или отмена — коллаут исчезает вместе с `PendingCallout`.
- Указатель рендерится стрелкой anchor→tip толщиной `size / 3` (render-and-hit).

## Кроп/ресайз (`FrameEdit`)

- `enter_frame_edit(resize)`: сбрасывает выделение; кроп стартует рамкой на весь кадр, ресайз — 75 % по центру. Хром: метка «W × H» и кнопка OK вдоль нижней кромки рамки (над ней, если кромка у низа окна; `layout_frame_chrome` на каждой тяге).
- Восемь ручек (`frame_spot_points`, углы первыми — приоритет попадания), допуск `HANDLE_RADIUS_PT + 3` в пикселях. Ручка у края окна прижимается внутрь только визуально; зона клика и курсор (`resetCursorRects`, crosshair) остаются на истинном месте.
- `frame_rect(orig, spot, start, cur)` — прямоугольник рамки при тяге ручки или (для кропа) центра, со standardize.
- `commit_frame_edit` (OK/Enter): кламп в кадр; рамка мельче пикселя не применяется — остаётся на экране; иначе `Command::Crop`/`Command::Resize`, сброс инструмента, `refit_window`. `cancel_frame_edit` (Esc) и `exit_frame_edit` (смена инструмента/снимка) — рамка снимается без команды.

## Палитра и слайдер

- `COLOR_PRESETS` — 9 цветов, дефолт magenta, есть чёрный и белый. Поповер цвета со поведением `ApplicationDefined`: закрывается только выбором пресета или повторным кликом по свотчу; `popoverDidClose` чистит слот (перед `performClose` слот забирается первым — иначе re-entry паникует на двойном borrow).
- Пресет красит новые элементы и перекрашивает выделенный (`Recolor`); блюр пропускается, чтобы undo не держало пустую команду. Слайдер (`widthChanged:`) задаёт толщину новых элементов и живо правит выделенный штриховой через `apply_width` (слияние тяги — doc-and-sessions); `sync_width_slider` на каждом redraw ставит ручку на толщину выделенного (диапазон 1..12, дефолт 4).
