# Хоткеи: регистрация, рекордер, миграция

Детализация к скиллу `minisnap-shell` (модули `src/hotkeys.rs`, `src/config.rs`, рекордер в `src/app.rs`). Норматив — спеки `openspec/specs/global-hotkeys/spec.md` и `openspec/specs/preferences/spec.md`; здесь только механика реализации.

## Рекордер (запись комбинации в настройках)

Правила записи — чистая машина в `src/hotkey_recorder.rs` (`Recorder`:
`begin(row) -> bool`, `feed(mods, keycode) -> FeedOutcome`, `recording_row()`;
`FeedOutcome = Recorded{row, hotkey} | Cancelled | Swallowed | Passthrough`),
состояние живёт в `AppIvars.recorder: RefCell<Recorder>`. Клей в `app.rs`:
клик кнопки → `prefsRowClicked:` (tag = `ROW_*`) → `begin` (false — уже
пишем другую строку или неизвестная строка, молча) → `update_prefs_row_titles`;
мониторный колбэк `recorded_key_event` распаковывает NSEvent (тип ≠ KeyDown —
насквозь), переводит флаги через `modifiers_from_flags` и скармливает `feed`:
`Passthrough` — событие проходит, остальное — глотается (null), а
`Cancelled`/`Recorded` до того вызывают `finish_recording` — запись в
`Config`, `save_to(standard_path())`, перерегистрация, `apply_menu_hotkey`,
`update_prefs_row_titles` (очистка состояния — внутри машины).

## Механика

- **Хоткеи опрашиваются, колбэков нет.** События крейта `global-hotkey` приезжают в его канал; тик дрейнит `pending_actions()` в главном потоке. Событие со старым id (комбинация уже заменена) дропается; обработчиков в чужих потоках нет.
- **Один источник правды комбинаций — `Config`**: из него регистрация, шорткаты меню (`key_equivalent_string` + `modifier_flags`) и кнопки настроек (`render_combo`) — разъехаться не могут.
- **Перерегистрация = пересоздание.** `HotkeyRegistry::apply` дропает старый `GlobalHotKeyManager` (это unregisters всё) и создаёт новый; занятая другой приложением комбинация логируется, остальные регистрируются.
- **Локальный key-monitor постоянный.** Ставится один раз при старте (`NSEvent::addLocalMonitorForEventsMatchingMask_handler(KeyDown)`), хранится в `AppIvars.monitor`; активен, только когда машина пишет (см. `recording_row()`), вне записи события проходят насквозь. В записи глотается всё: неизвестный keycode (запись продолжается), зафиксированная клавиша. Комбинации нужен хотя бы один модификатор: голая клавиша (кроме отменяющего Esc) глотается, запись продолжается — норматив в спеке `preferences`.
- **Миграция легаси одноразовая.** `migrate_legacy_defaults` срабатывает только в файле без ключа `capture_edit_hotkey`: прежний дефолт capture переносится на новое значение, capture-edit занимает освободившийся старый дефолт (константы — `legacy_capture_hotkey` и дефолты `Config`); кастомная комбинация не трогается; `load_or_create` один раз перезаписывает файл.
