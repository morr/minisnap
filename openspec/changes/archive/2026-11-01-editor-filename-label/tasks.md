# Tasks

## 1. Подпись имени файла в тулбаре

- [x] 1.1 Восстановить лейбл в `src/editor_window.rs` по design.md: ivar `file_label`, создание в построении тулбара (`NSTextField::labelWithString`, `secondaryLabelColor`, `ByTruncatingMiddle`, сопротивление сжатию 250 по горизонтали), вставка в стек после пары листания с `setCustomSpacing(BAR_ZONE_SPACING, label)`, сохранение в ivars рядом с `tool_buttons`/`width_slider`. Проверка: `cargo build` чист.
- [x] 1.2 Запись имени в `display_shot`: перед `refit_window` выставить `setStringValue` с именем файла показанного снимка. Проверка: `MINISNAP_DEBUG_OPEN_EDITOR=<path>` — при открытии в полосе после ◄/► показано имя файла; листание ◄/► меняет подпись на имя соседнего снимка; drag за подпись и свободное место полосы переносит окно; `cargo build` чист.
- [x] 1.3 Блок имени фиксированной ширины с центрированным текстом: константа `FILE_LABEL_WIDTH` (276 pt = самое широкое типовое имя 251 pt при 13 pt + 10%), `setTranslatesAutoresizingMaskIntoConstraints(false)` + констрейнт `widthAnchor`, `NSTextAlignment::Center`, сопротивление сжатию 250 остаётся. Проверка: `cargo build` чист; через Accessibility фрейм подписи при двух снимках с именами разной длины одинаков (276 pt), контролы правее не сдвигаются.
- [x] 1.4 Разделитель между парой листания и блоком имени: `bar_separator` сразу после `leaf_buttons[1]`, интервалы зоны по обе стороны. Проверка: `cargo build` чист; в полосе визуально — листание | имя | цвет+толщина | инструменты.

## 2. Документация

- [x] 2.1 Обновить скилл `.zcode/skills/minisnap-editor/SKILL.md`: во вводке заменить «имени файла в окне нет» на подпись в первой зоне полосы после пары листания, обновляющуюся в `display_shot`. Проверка: текст скилла соответствует коду.

## Workflow follow-up

- Архивировать change после проверки (`/opsx:archive`).
