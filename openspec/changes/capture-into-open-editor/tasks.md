# Tasks

## 1. Маршрутизация в `src/app.rs`

- [x] 1.1 `MainJob::OpenEditor` получает поле `only_if_visible: bool`; оба конструктора обновлены: колбэк `OverlayOutcome::Selected` шлёт `only_if_visible = !edit`, debug-хук `MINISNAP_DEBUG_OPEN_EDITOR` — `false`. Проверка: `cargo check` чист.
- [x] 1.2 Обработчик в `tick:` при `only_if_visible == true` и невидимом окне редактора (`editor` отсутствует либо `window.isVisible() == false`) пропускает задачу со строкой журнала; при видимом или без условия — прежний `open_editor(path)`. Проверка: `cargo check` чист.

## 2. Живая проверка и скиллы

- [x] 2.1 Пересобрать бандл `scripts/build_app.sh release`, перезапустить инстанс и проверить по MCP minisnap все три сценария дельты: редактор ни разу не открывался → `Capture{edit:false}` не открывает окно; `CloseEditor` после открытия → `Capture{edit:false}` окно не поднимает; окно открыто → `Capture{edit:false}` показывает новый снимок в том же окне и выводит его на передний план, как `Capture{edit:true}`. Проверка: `state` (видимость окна редактора) и `log` после каждого шага; служебные окна спрятать после проверки.
- [ ] 2.2 `cargo test` проходит (юнит-тесты не добавляются: изменение — AppKit-маршрутизация в `app.rs`, вне окон не тестируется); `git diff` подтверждает, что пайплайн вывода в `capture_ready` (кроп, звук, буфер, сохранение) не менялся. Проверка: вывод `cargo test` и `git diff` без правок в ветке вывода.
- [ ] 2.3 Доменные скиллы отражают новое условие: `minisnap-architecture` (шаг 4 сквозного потока — условие отправки `MainJob::OpenEditor`) и `minisnap-shell` (секция «Тик-насос» — условие в обработке `OpenEditor`). Проверка: соответствующие строки скиллов описывают `only_if_visible`-условие.

## Workflow follow-up

- Архивировать change (`/opsx:archive`) после проверки живого поведения.
