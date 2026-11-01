# Tasks

## 1. Метка чекаута

- [x] 1.1 В `src/app.rs` добавить чистую `fn checkout_label(exe: &Path) -> String` (сканирование компонентов пути: пара `.worktrees/<branch>` → ветка, иначе `main`) с юнит-тестами: worktree-путь, путь основного чекаута, путь без `.worktrees`. Проверка: `cargo test` зелёный.
- [x] 1.2 В `app::run` вычислить метку через `instance_identity()`, напечатать в stderr `minisnap instance '<метка>' from <путь бинарника>` до создания контроллера, передать метку в `AppController::new` и сохранить в ivars `checkout_label`. Проверка: запуск бинарника печатает строку; `cargo build` чист.

## 2. Трей-меню и tooltip

- [ ] 2.1 В `AppController::new` перед «Capture Frozen Area» добавить заголовочный пункт «minisnap — <метка>» (без action, `setEnabled(false)`) и разделитель после него. Проверка: меню открывается, заголовок виден и некликабелен, порядок и состав остальных пунктов прежние; `cargo build` чист.
- [ ] 2.2 Выставить кнопке статус-итема tooltip «minisnap — <метка>». Проверка: наведение на иконку трея показывает tooltip; `cargo build` чист.

## 3. Заголовок окна настроек

- [ ] 3.1 `prefs::build` принимает метку (`&str`) и ставит заголовок окна «Preferences — <метка>»; вызов в `app.rs` передаёт метку из ivars. Проверка: у инстанса из worktree окно настроек озаглавлено «Preferences — <ветка>», у инстанса из основного чекаута — «Preferences — main»; `cargo build` чист.

## 4. Документация

- [ ] 4.1 Обновить скилл `.zcode/skills/minisnap-shell/SKILL.md`: в карте модулей `checkout_label`/`instance_identity` в app.rs, поток запуска со строкой в stderr, состав меню с заголовочным пунктом, tooltip, параметр метки в `prefs::build`. Проверка: текст скилла соответствует коду.
- [ ] 4.2 Дополнить раздел «Working with a running app» в AGENTS.md: метка чекаута в меню/tooltip'е/заголовке настроек и строка в stderr служат перекрёстной проверкой к опознанию по `ps`; `ps -axo pid,comm` остаётся истиной в последней инстанции. Проверка: текст согласован с кодом.

## Workflow follow-up

- Архивировать change после проверки (`/opsx:archive`).
