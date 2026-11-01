---
name: minisnap-output
description: Map of the minisnap output domain (src/output.rs, src/sound.rs) — clipboard copy of a finished shot, silent PNG saving with timestamped collision-safe file names, retention of old shots, listing shots for open-latest and editor paging, and the system shutter sound played when a capture completes. Use for tasks about copy_to_pasteboard, save_to_file, shot file naming or recognition, retention limits, latest_shot/sorted_shots listings, or play_shutter and the disable-sound system switch. Not for shell config, capture/overlay, or editor UI.
---

# Домен output — вывод результата захвата

`src/output.rs` (~210 строк прод-кода плюс тесты) превращает завершённое выделение в результат: кроп кадра T0, копия в буфер обмена, тихое сохранение PNG с именем по времени, ретенция папки и листинг снимков. `src/sound.rs` — звуковой фидбек того же момента: системный щелчок затвора по завершении выделения. Всё работает с уже готовым `CGImage`; захват и координаты — домен `minisnap-capture`, окно и инструменты редактора — `minisnap-editor`. Нормативное поведение — спека `capture-output`.

## Карта модулей

### `src/output.rs`

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

### `src/sound.rs`

| Функция | Вид | Что делает |
|---|---|---|
| `play_shutter` | pub | Системный щелчок затвора после успешного кропа; best-effort, ошибок не возвращает; только главный поток (`MainThreadMarker::new()`), иначе тихий выход |
| `prewarm` | pub | Прогрев аудио на старте (через одноразовый NSTimer в `app::run`): pref-read + загрузка плеера + беззвучный `setVolume(0)`→`play`→`stop`; без него первый в процессе `play` стоит ~0.8 с (инициализация CoreAudio) и задерживает и звук, и fade-out оверлея |
| `shutter_sound_paths` | приват | Цепочка системных файлов звука, лучшие вперёд: `Shutter.aif` в CoreAudio.component (macOS 26+), затем `camera-shutter.aiff` в ScreenCapture.framework (старые версии) |
| `sound_disabled_by_system` | приват | Чтение ключа `disable-sound` домена `com.apple.screencapture` (`CFPreferencesCopyAppValue`); ключа нет/нечитаем → звук включён |
| `load_player_from` | приват | Первый существующий файл цепочки → `NSSound`; неудача загрузки — stderr |
| `play_shutter_from` | приват | Воспроизведение по цепочке путей (вынесено для тестов деградации); `PLAYER` — переиспользуемый `NSSound` в `thread_local` |

## Поток данных

- **Захват завершён** (`src/app.rs`, callback `OverlayOutcome::Selected`): `crop` → пустой результат — только лог и выход; далее `sound::play_shutter()` (звук до буфера и файла — «снимок сделан», а не «файл записан») → `copy_to_pasteboard` (ошибка логируется, сохранение всё равно идёт) → `save_to_file(&image, &save_dir, retention, scale)`, scale — backing scale дисплея захвата. `save_dir` и `retention` берутся из `Config` (`src/config.rs`; дефолты `~/Pictures/minisnap` и 100). При `edit`-режиме путь уезжает в `MainJob::OpenEditor` через тик.
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
- **Звук затвора — не ассет приложения, а системный файл по цепочке путей** (`shutter_sound_paths`); путь переезжает между релизами macOS, первый существующий файл берётся, ничего не нашлось — снимок продолжается без звука. Собственного выключателя в config нет: honored системный ключ `com.apple.screencapture disable-sound`, тот же, что глушит звук системной утилиты скриншотов.
- **Первый в процессе `play` — ~0.8 с инициализации CoreAudio, холодный pref-read — ~0.15 с** (замеры в `#[ignore]`-тесте `timing_report`); поэтому `prewarm` на старте обязателен для нового процесса, а не опция — без него первый снимок держит главный поток (общий с fade-out оверлея) почти секунду. Тёплые `play` — микросекунды.
- **`NSSound` в `thread_local`-слоте**: объект, уроненный сразу после `play`, обрезает звук, поэтому инстанс кешируется и переиспользуется; повторный `play` на том же инстансе перезапускает звук с начала — наложений при быстрых снимках нет. Все вызовы из главного потока (обработчик `Selected` в тике).

Скрытые договорённости с другими доменами (пиксельное пространство координат, владение `CGImage`, главный поток) — в скиле `minisnap-architecture`; перед правкой сверяйся с кодом по адресам выше.
