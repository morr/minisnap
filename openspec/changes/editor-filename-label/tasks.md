# Tasks

## 1. Подпись имени файла в тулбаре

- [ ] 1.1 Восстановить лейбл в `src/editor_window.rs` по design.md: ivar `file_label`, создание в построении тулбара (`NSTextField::labelWithString`, `secondaryLabelColor`, `ByTruncatingMiddle`, сопротивление сжатию 250 по горизонтали), вставка в стек после пары листания с `setCustomSpacing(BAR_ZONE_SPACING, label)`, сохранение в ivars рядом с `tool_buttons`/`width_slider`. Проверка: `cargo build` чист.
- [ ] 1.2 Запись имени в `display_shot`: перед `refit_window` выставить `setStringValue` с именем файла показанного снимка. Проверка: `MINISNAP_DEBUG_OPEN_EDITOR=<path>` — при открытии в полосе после ◄/► показано имя файла; листание ◄/► меняет подпись на имя соседнего снимка; drag за подпись и свободное место полосы переносит окно; `cargo build` чист.

## 2. Документация

- [ ] 2.1 Обновить скилл `.zcode/skills/minisnap-editor/SKILL.md`: во вводке заменить «имени файла в окне нет» на подпись в первой зоне полосы после пары листания, обновляющуюся в `display_shot`. Проверка: текст скилла соответствует коду.

## Workflow follow-up

- Архивировать change после проверки (`/opsx:archive`).
