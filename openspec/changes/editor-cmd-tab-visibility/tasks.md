# Tasks

## 1. Переключение политики активации (app.rs, editor_window.rs)

- [x] 1.1 В `app.rs`: хелпер `sync_activation_policy` (Regular ⇔ редактор `isVisible`, иначе Accessory) и вызов в `open_editor` после `show_shot`; `cargo build --release` проходит, при запуске без редактора поведение не меняется (проверка: приложение стартует агентом, `state` прежний).
- [ ] 1.2 В `app.rs`: конформность `NSWindowDelegate` у `AppController` (паттерн как у `NSApplicationDelegate`), `setDelegate` на окне редактора, `windowWillClose:` → синхронизация при скрытии; в `editor_window.rs`: `miniaturize:`/`performMiniaturize:` переводятся с `orderOut` на `close()`. Проверка: `cargo build --release` проходит; окно редактора после жёлтой кнопки и `CloseEditor` прячется как прежде, повторный `show_shot` поднимает его.

## 2. Политика активации в агент-канале

- [x] 2.1 В `agent_proto.rs` — поле `activation_policy` в payload `/state`, заполнение в `handle_state` (`src/app.rs` / `src/agent_server.rs`). Проверка: `cargo build --release` проходит; на запущенном приложении `GET /state` до открытия редактора отдаёт `accessory`.
- [ ] 2.2 Мост `src/bin/minisnap-mcp.rs`: строка политики в человекочитаемом тексте `state`, если формат текста её не подхватывает сам; пересборка моста. Проверка: `state` MCP-инструмента показывает `accessory` при скрытом редакторе.

## 3. Live-проверка на пересобранном приложении

- [ ] 3.1 Пересборка бандла `scripts/build_app.sh release`, перезапуск инстанса; через MCP: `OpenEditor` → `state` показывает `regular`; `CloseEditor` → `state` показывает `accessory`. Кросс-проверка извне: `lsappinfo` показывает смену политики активации процесса. Ручной шаг: клик жёлтой кнопки пользователем — окно прячется, `state` снова `accessory`.
- [ ] 3.2 Визуальная проверка cmd+tab пользователем: открытый редактор присутствует в переключателе и переключение поднимает окно; после закрытия minisnap из переключателя исчезает.

## 4. Доменные скилы

- [ ] 4.1 Скил `minisnap-shell`: механизм синхронизации политики активации (хелпер, точки вызова), грабля о задержке Dock-иконки при Accessory→Regular. Проверка: текст скила соответствует коду (адреса и имена сверены).
- [ ] 4.2 Скил `minisnap-editor`: единая воронка скрытия — все пути скрытия через `close` → `windowWillClose:` (в т.ч. перевод миниатюризации), делегат окна — `AppController`. Проверка: текст скила соответствует коду.

## Workflow follow-up

- `/opsx:archive` после проверки: merge ветки в master и снос worktree, если работа шла в worktree.
