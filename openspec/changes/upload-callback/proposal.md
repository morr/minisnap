# Proposal

## Why

Интерфейс трейта `UploadBackend` включает «отчитайся `MainJob`-ом»: `upload(&self, png, name, jobs: Sender<MainJob>)`, и домен вывода импортирует перечисление задач оболочки (`upload.rs: use crate::app::MainJob`). Шов направлен вверх — от вывода к оболочке; второму потребителю загрузки или тесту пришлось бы фабриковать `MainJob`.

## What Changes

- Трейт `UploadBackend` докладывает исход один раз через `on_done: Box<dyn FnOnce(Result<String, String>) + Send>` вместо канала `Sender<MainJob>`; зависимость `upload.rs → app::MainJob` исчезает.
- `upload_file` принимает и прокидывает тот же колбэк; ретрай-цепочка 409 остаётся внутренней реализацией `ArtifactsBackend` — колбэк вызывается один раз, в конце цепочки.
- `app.rs` оборачивает доклад в существующий насос: колбэк отправляет `MainJob::UploadFinished` в канал задач — тик перекачивает его на главный поток как сегодня; исполнение колбэка на очереди сессии NSURLSession безопасно для `mpsc::Sender`.
- Трейт остаётся: `build_backend` реально ветвится по `config.upload_provider`, поведение протестировано — в отличие от декоративного реестра настроек.

Чистая перестановка шва: пользовательски наблюдаемое поведение (кнопка в состоянии загрузки, URL в буфере и логе, диалог ошибки) не меняется — спеки не затрагиваются.

## Capabilities

### New Capabilities

(нет)

### Modified Capabilities

(нет — `shot-upload` закрепляет поведение загрузки, а не механизм доклада; change объявляет `skip_specs: true`)

## Impact

- Код: `src/upload.rs` (интерфейс трейта и `upload_file`), `src/app.rs` (обёртка колбэка в `start_upload`).
- Спеки: не меняются (`skip_specs: true`).
- Доменный скилл `minisnap-output` обновляется тем же change'ем (если change 3 уже прошёл — правка в его новой карте).
