# Design

## Подход

Перестановка раскладки без смены стека и механики: панель, канва, кнопки, hover, поповер и хром рамки — те же объекты, меняются их позиции, порядок в стеке и обвязка раскладки. Всё в `src/editor_window.rs`. Стиль опирается на вердикт базы «Minimalism & Swiss Style» (чистые зоны, только существенное, высокая контрастность) и наследует фиксы аудита `editor-restyle-audit` (подписи доступности, кроссфейд плашек с гейтом reduced-motion, свотчи 24 pt).

## Решения

### Полоса сверху (вместо плавающей пилюли снизу)

- Панель `ToolbarPanel` (NSVisualEffectView, `.hudWindow`, тёмный appearance окна — без изменений) становится сплошной полосой: frame `(0, H − TOOLBAR_HEIGHT, W, TOOLBAR_HEIGHT)`, autoresizing `ViewWidthSizable | ViewMinYMargin` (прижата к верху). Скругление панели (`layer.cornerRadius = WINDOW_RADIUS`, `masksToBounds`) убирается: верхние углы скругляет маска контент-вью окна, а нижние углы полосы должны быть прямыми — иначе выемка против канвы.
- Канва `EditorView`: frame `(0, 0, W, H − TOOLBAR_RESERVED)`, autoresizing `ViewWidthSizable | ViewHeightSizable` (растёт от низа). `TOOLBAR_RESERVED` становится `TOOLBAR_HEIGHT` (полоса без отступов — интегрированная), `TOOLBAR_MARGIN` уходит, горизонтальные внутр. отступы стека — новая константа `BAR_PAD = 12`.
- Hairline под полосой: тонкая полоска 1 pt на всю ширину у нижнего края панели (`wantsLayer`, фон белый 12% — константа `BAR_HAIRLINE`), autoresizing `ViewWidthSizable`. Отделяет вибрацию от канвы, единственная новая графика.
- Стек: frame `(BAR_PAD, 0, W − 2·BAR_PAD, TOOLBAR_HEIGHT)`, autoresizing `ViewWidthSizable | ViewHeightSizable`.
- `refit_window`: кадр вписывается в `(visible.width, visible.height − TOOLBAR_RESERVED)` — логика та же, значение константы новое; начальная высота окна `INITIAL_SIZE.height + TOOLBAR_RESERVED`.
- `min_width = toolbar.fittingSize().width + 2·BAR_PAD`.
- Перетаскивание окна: `ToolbarPanel.mouseDown` → `performWindowDrag` — работает как раньше; клики по контролам гасятся контролами.

### Три зоны и разделители

- Порядок в стеке: `[◄, ►, имя файла] | SEP | [стрелка, прямоугольник, линия, pen] · [текст, коллаут, блюр] · [кроп, ресайз] | SEP | [свотч, слайдер]`. Два разделителя — не больше: минимализм, зоны читаются тремя блоками.
- Разделитель: `NSView` 1×22 pt, `wantsLayer`, фон белый 15% (константа `BAR_DIVIDER`). Хелпер `bar_separator()`; в `hover_at` он не NSButton — хит-тест вернёт None, подсветки не требует.
- Интервалы: базовый spacing стека 6 pt (как сейчас); между зоной и разделителем — кастом-интервал 10 pt (`setCustomSpacing_afterView` на последнем элементе зоны и на разделителе); между группами инструментов — кастом-интервал 10 pt без разделителя. Существующий `setCustomSpacing_afterView(14.0, &file_label)` заменяется этой схемой.
- Имя файла: `secondaryLabel`, обрезка по середине, compression resistance — как сейчас (просто живёт в левой зоне).

### Что не меняется

- Материал, appearance, `WINDOW_RADIUS` окна, фон `BACKDROP`, тины/плашки/анимации (`hover_at`, `update_tool_highlight`, `animate_plate_changes`), свотчи 24 pt, слайдер 90×24 горизонтальный, подписи доступности и тултипы, поповер палитры (раскрывается от свотча вниз, на канву — корректно для верхней полосы), OK-кнопка и размерная подпись рамки (субвью канвы), хит-тесты, клавиатура.

## Риски

- None блокирующие. Совместное поведение панель+канва по autoresizing проверяется прогоном: при росте окна полоса прижата к верху, канва растёт от низа.
