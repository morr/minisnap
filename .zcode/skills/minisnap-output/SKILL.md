---
name: minisnap-output
description: Map of the minisnap output domain (src/output.rs) — clipboard copy of a finished shot, silent PNG saving with timestamped collision-safe file names, retention of old shots, and listing shots for open-latest and editor paging. Use for tasks about copy_to_pasteboard, save_to_file, shot file naming or recognition, retention limits, or latest_shot/sorted_shots listings. Not for shell config, capture/overlay, or editor UI.
---

# Домен output — вывод результата захвата

`src/output.rs` (~210 строк прод-кода плюс тесты) превращает завершённое выделение в результат: кроп кадра T0, копия в буфер обмена, тихое сохранение PNG с именем по времени, ретенция папки и листинг снимков. Всё работает с уже готовым `CGImage`; захват и координаты — домен `minisnap-capture`, окно и инструменты редактора — `minisnap-editor`. Нормативное поведение — спека `capture-output`.

## Карта модуля

| Функция | Вид | Что делает |
|---|---|---|
| `crop` | pub | Кроп `CGImage` по `CGRect` в пиксельном пространстве с верхним левым началом; `standardize()` + `None` для пустого rect |
| `copy_to_pasteboard` | pub | PNG + TIFF в general pasteboard; только главный поток (`MainThreadMarker`), иначе `Err` |
| `save_to_file` | pub | Тихое сохранение: `create_dir_all` → `unique_shot_path` → `encode_png` → `enforce_retention`; принимает backing scale захвата — при `scale > 1` имя получает маркер плотности `@<n>x` перед расширением; возвращает путь |
| `enforce_retention` | pub | Держит ≤ `limit` снимков, удаляет самые старые; best-effort, неудалённый файл логируется в stderr |
| `latest_shot` | pub | Самый новый снимок папки (`shot_files_oldest_first().pop()`); пустая/несуществующая папка → `None` |
| `sorted_shots` | pub | Все снимки папки, старые вперёд; несуществующая папка → пустой вектор |
| `encode_png` | pub(crate) | Кодирование через ImageIO `CGImageDestination` (`public.png`) |
| `encode_png_atomic` | pub(crate) | `encode_png` в sibling-файл `.tmp`, затем `fs::rename` — крах посреди записи не оставляет половину файла |
| `timestamp_components` | приват | Календарные компоненты Unix-времени со сдвигом (civil-from-days, Hinnant) |
| `shot_base_name` | приват | `MiniSnap_YYYY-MM-DD_HH-MM-SS` в локальном времени; подчёркивания, без пробелов |
| `unique_shot_path` | приват | Первый свободный путь: при коллизии в ту же секунду `_2`..`_999`, дальше fallback на наносекунды; суффикс коллизии всегда последний — после маркера плотности (`...@2x_2.png`) |
| `local_utc_offset_secs` | приват | Системный offset UTC в секундах (`NSTimeZone::systemTimeZone`) |
| `is_shot_file_name` | приват | «Наш» ли файл: префикс `MiniSnap_` или legacy `MiniSnap ` (пробел) + суффикс `.png` |
| `shot_files_oldest_first` | приват | Чтение папки через `is_shot_file_name`; сортировка по mtime, имя — tie-break |

## Поток данных

- **Захват завершён** (`src/app.rs`, callback `OverlayOutcome::Selected`): `crop` → пустой результат — только лог и выход; далее `copy_to_pasteboard` (ошибка логируется, сохранение всё равно идёт) → `save_to_file(&image, &save_dir, retention, scale)`, scale — backing scale дисплея захвата. `save_dir` и `retention` берутся из `Config` (`src/config.rs`; дефолты `~/Pictures/minisnap` и 100). При `edit`-режиме путь уезжает в `MainJob::OpenEditor` через тик.
- **Редактор правит снимок** (`src/editor/session.rs:100`, `OpenShot::persist` после каждой команды/undo/redo): `render_composite` → `encode_png_atomic(&composite, &self.shot_path)` — файл снимка перезаписывается на месте, атомарно. Оригинал сессии пишется тем же `encode_png` через собственный `write_atomic` сессии (session.rs:162).
- **⌘C в редакторе** (`src/editor_window.rs:1164`, `copy_composite`): композит без маркеров → `copy_to_pasteboard`.
- **«Открыть последний»** (`src/app.rs:628`, `open_latest`): `latest_shot(&config.save_dir)` → `open_editor`; пустая папка — тихий no-op.
- **Листание в редакторе** (`src/editor_window.rs:1756`, `page`): `sorted_shots(&save_dir)`, позиция текущего файла, `clamp` на края.
- **Кроп внутри редактора**: `Command::Crop` (`src/editor/mod.rs:250`) и пикселизация блюра (`src/editor/render.rs:166`) переиспользуют `crop`.

## Инварианты и грабли

- **Порядок — по mtime, не по имени.** `shot_files_oldest_first` сортирует по времени модификации (имя — только tie-break). Лексикографика имён совпадает с хронологией лишь пока поля фиксированной ширины и offset один и тот же; смена таймзоны (DST) ломает порядок имён — поэтому код на имена не полагается. Внешние потребители имён (Finder) могут увидеть «неправильный» порядок после смены таймзоны.
- **`save_to_file` пишет НЕ атомарно** — прямой `encode_png`, без `.tmp`+rename; атомарная версия (`encode_png_atomic`) используется только сессиями редактора. Крах при первом сохранении может оставить недописанный PNG, который при этом уже попадёт под ретенцию/листинг.
- **Маркер плотности `@<n>x` — единственный носитель плотности снимка.** Пишется при сохранении по backing scale захвата; редактор читает его `name_density` (`src/editor/session.rs`) и показывает кадр в 1/n — размером с экран в момент съёмки. Файлы без суффикса (старые снимки, чужие файлы) показываются 1:1; суффикс переживает очистку tmp-сессий и листание папки, распознавание «своих» файлов (`is_shot_file_name`) ему не мешает.
- **Коллизия имён**: две секунды — два разных имени; та же секунда → `_2`, `_3`… до `_999`, дальше наносекундный хвост. Это единственный механизм уникализации, cleanup старых имён не связан с нумерацией.
- **Распознавание «своих» файлов грубое**: префикс + `.png`, без проверки формата даты. `MiniSnap_notes.png` посчитается снимком и может быть удалён ретенцией; `MiniSnap notes.txt` — нет (не PNG). Legacy-пробел (`MiniSnap 2026-…`) матчится специально — ретенция чистит файлы старых сборок. Чужие файлы ретенцией не трогаются никогда.
- **Ретенция best-effort**: неудачное удаление логируется и пропускается (`eprintln!`), лимит не «догоняется» на следующем сохранении лишней агрессией — просто пересчитывается заново.
- **Имя в локальном времени**: offset берётся в момент сохранения (`local_utc_offset_secs`); файлы, созданные до и после перевода часов, сортируются корректно только благодаря mtime.
- **`copy_to_pasteboard` — главный поток**, кладёт PNG и TIFF одновременно (TIFF — для старых приложений); частичный отказ pasteboard не отменяет сохранение в файл, и наоборот.
- **Ретенция срабатывает после каждого сохранения**, лимит — `config.retention` (дефолт 100, спека `capture-output` требует именно 100 максимум).

Скрытые договорённости с другими доменами (пиксельное пространство координат, владение `CGImage`, главный поток) — в скиле `minisnap-architecture`; перед правкой сверяйся с кодом по адресам выше.
