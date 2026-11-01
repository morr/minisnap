# Хоткеи: регистрация, рекордер, миграция

Детализация к скиллу `minisnap-shell` (модули `src/hotkeys.rs`, `src/config.rs`, рекордер в `src/app.rs`). Норматив — спеки `openspec/specs/global-hotkeys/spec.md` и `openspec/specs/preferences/spec.md`; здесь только механика реализации.

## Рекордер (запись комбинации в настройках)

Клик кнопки → `prefsRowClicked:` (tag = `ROW_*`) → `start_recording(row)`; следующий keydown ловит локальный монитор → `recorded_key_event` (Esc отменяет запись) → `finish_recording` — запись в `Config`, `save_to(standard_path())`, перерегистрация, `apply_menu_hotkey`, `update_prefs_row_titles`.

## Механика

- **Хоткеи опрашиваются, колбэков нет.** События крейта `global-hotkey` приезжают в его канал; тик дрейнит `pending_actions()` в главном потоке. Событие со старым id (комбинация уже заменена) дропается; обработчиков в чужих потоках нет.
- **Один источник правды комбинаций — `Config`**: из него регистрация, шорткаты меню (`key_equivalent_string` + `modifier_flags`) и кнопки настроек (`render_combo`) — разъехаться не могут.
- **Перерегистрация = пересоздание.** `HotkeyRegistry::apply` дропает старый `GlobalHotKeyManager` (это unregisters всё) и создаёт новый; занятая другой приложением комбинация логируется, остальные регистрируются.
- **Локальный key-monitor постоянный.** Ставится один раз при старте (`NSEvent::addLocalMonitorForEventsMatchingMask_handler(KeyDown)`), хранится в `AppIvars.monitor`; активен только при `recording != None`, вне записи события проходят насквозь. В записи глотается всё: неизвестный keycode (запись продолжается), зафиксированная клавиша; пустые модификаторы валидны — голая клавиша станет глобальным хоткеем.
- **Миграция легаси одноразовая.** `migrate_legacy_defaults` срабатывает только в файле без ключа `capture_edit_hotkey`: прежний дефолт capture переносится на новое значение, capture-edit занимает освободившийся старый дефолт (константы — `legacy_capture_hotkey` и дефолты `Config`); кастомная комбинация не трогается; `load_or_create` один раз перезаписывает файл.
