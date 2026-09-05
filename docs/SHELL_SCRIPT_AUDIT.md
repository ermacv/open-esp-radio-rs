# Аудит shell-скриптов и параллельных сбоев

Исторический снимок до переноса автоматизации в Rust. Прежние команды и
результаты ниже сохранены; текущие замены перечислены в
[карте xtask](../tools/repo/README.md). Linux/OpenWrt HIL scripts остаются
стендовыми инструментами. Старые PASS не являются результатами нового gate.

Дата: 2026-09-05. Область: существующие tracked и nonignored файлы
репозитория с расширением `.sh`/`.bash` либо shell shebang, включая файлы без
расширения. Найдено 11 скриптов: семь `.sh`, четыре без расширения.
Локальные build outputs и private `_oracles` не входят в исходный код.

## Причина сбоев HIL и Blobray

Оба сбоя после drop → immediate reacquire воспроизведены с настоящим `fork`.
Дочерний процесс наследует дескриптор той же open file description. Закрытие
родительского `File` оставляло `flock` активным, пока ребёнок не закроет свою
копию. `CLOEXEC` не закрывает промежуток между `fork` и `exec`.
Это соответствует семантике [Linux flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html).

- HIL `FixtureLock` явно вызывает unlock при завершении владения. Guard
  создаётся сразу после успешного захвата, до потенциальных ошибок записи
  диагностических метаданных.
- Blobray разделяет `Arc<AccessLock>` между snapshot и временными read-only
  views. Последний логический владелец вызывает unlock после закрытия SQLite
  и завершения cleanup. Дублирование дескриптора для проверки identity само
  по себе больше не продлевает срок блокировки.
- Три fork-регрессии падают на прежнем production-коде и проходят на
  исправленном. Дополнительный тест запрещает преждевременный unlock после
  временного запроса при живом snapshot. В дочерних процессах выполняются
  только async-signal-safe операции, без Rust destructors и unwinding.

В исправлениях нет lock retries, задержек или сериализации тестов. Прогон с
`--test-threads=1` был диагностическим и не считается устранением причины.
Regression tests находятся отдельно от production-кода:
[HIL](../hil/host/runner/src/lab/lock/tests.rs),
[Blobray](../tools/blobray/src/application/query_store/lock_tests.rs).

## Назначение и решение для каждого скрипта

| Скрипт | Что проверяет или делает; реальный потребитель | Решение |
| --- | --- | --- |
| `audit-source-only.sh` | Общий source-only gate из README/AGENTS: Cargo, Clippy, PAC, qualification, ABI и конечный ELF | Сохранён; сетевой gate включён, анализ ELF ограничен через launcher |
| `audit-cargo-metadata.sh` | Обнаруживает действительные Cargo workspace roots и проверяет lockfiles; вызывается source-only, проверяется Python regression tests | Оставить: root Cargo не покрывает независимые workspaces |
| `audit-driver-architecture.sh` | Компилирует feature profiles, проверяет Cargo boundaries и composition tests; вызывается source-only | Сохранён; вложенные `target` исключены из поиска manifests |
| `audit-driver-safety.sh` | Compiler-enforced unsafe policy, reviewed PAC consumers, ownership tests; вызывается source-only | Оставить: явные allowlists обозначают reviewed authority |
| `check-network-adapter-boundaries.sh` | Owned/compat/research/runtime dependencies и compile profiles; ранее только ручной запуск из WIFI_EGRESS_STATUS | Сохранён; graph policy переписана, source spelling удалён, лёгкий режим включён в source-only |
| `check-esp32s31-examples.sh` | Target `cargo check` четырёх independent examples и compat-варианта; вызывается source-only | Оставить; не называть этот результат link/run evidence |
| `check-standalone` | Копирует generic Blobray в отдельный workspace, проверяет локальные пути и компилирует все targets | Сохранён как ручная portability check, выполнен и описан в Blobray README |
| `run-limited` | Ограничивает память/время process tree; используется при настоящем Blobray analysis и проверяется launcher test | Сохранён; отказ мониторинга и завершение при отмене исправлены |
| `build-analysis-inputs` | Собирает три Rust comparison ELF; ручной workflow, declaration CLI используется host test | Сохранён и описан; теперь Cargo parallelism по умолчанию, явный лимит через env |
| [install.sh](../hil/host/linux-net/install.sh) | Устанавливает fixture helper, patched hostapd, capability и ограниченные sudo-команды; ручная настройка из HIL README | Оставить отдельно от source audit |
| [open-radio-net](../hil/host/linux-net/open-radio-net) | Выполняет конечный список операций Linux fixture; вызывается типизированным HIL transport через установленный helper | Оставить: наблюдает реальное association/channel state |

Отсутствие автоматического caller не делает ручной инструмент устаревшим.
В checkout нет CI workflow YAML, Cargo aliases запускают Rust binaries.
Внешний CI и личные команды пользователей этим аудитом не покрываются.
Доказанно ненужных скриптов не найдено; лишней оказалась часть проверок.

## Удалённые и заменённые проверки

Из network gate удалён поиск `OwnedNetworkTxFrame|DatapathTxConsumer` в Rust
исходниках. Комментарий вызывал ложную ошибку, alias позволял обойти проверку.
Такой поиск не доказывает ни тип зависимости, ни владельца объекта.

Regex по `Cargo.toml` и форматированному `cargo tree` заменены Cargo metadata:
package IDs, manifest paths, source/version и достижимые normal/build edges.
Dev-only ветки не являются production зависимостями; dependency rename не
меняет identity пакета. Ошибки Cargo/JSON/schema приводят к отказу проверки,
а не к пустому списку нарушений через прежнее `jq ... || true`.

Cargo metadata объединяет features участников workspace. Поэтому граф
каждой из девяти проверяемых конфигураций получает отдельный временный
consumer с нужными features, workspace patches и копией исходного lockfile.
Временный lockfile инициализируется offline; все реальные package/source/
version остаются в исходном каталоге. Затем обязателен проход `--locked`.
Исходные manifests и lockfiles не переписываются. Неиспользованные patches
Cargo может записывать в разном порядке: из временного manifest удаляются
только overrides, которые Cargo объявил unused, с проверкой тождественности
графа до и после удаления. Повторных попыток замаскировать отказ нет.

Четырнадцать `тестов сетевого аудита`
включают настоящие Cargo workspaces: чужой member не должен включать chip
feature у проверяемого consumer, relative patches должны сохранять identity,
а изменение версии относительно исходного lock должно отклоняться.
Declared dependency policy отдельно учитывает отключённые optional edges:
нейтральный контракт не может спрятать production dependency за feature.

Внутренние границы модулей одного crate не выводятся из Cargo DAG. Их
поддерживают приватность, типизированные interfaces и ownership tests;
graph gate не заявляет, что способен запретить конкретный Rust type в роли.
Architecture gate проверяет выбранные dependency profiles: компиляция всех
заявленных feature profiles сама по себе не доказывает policy каждого графа.

`build-analysis-inputs --list-roles` — декларация CLI, а не свидетельство
производства ELF. Проверка связывает объявленные роли с configuration
потребителя; подлинность и пригодность конкретных файлов устанавливаются
отдельным vendor workflow. Успешный тест декларации не является MATCH.

В watchdog исправлены ещё два пути отказа. Ранее ошибка получения RSS
подавлялась через `|| true` и превращалась в нулевую память; ошибка получения
списка процессов могла выглядеть как завершение сессии. Теперь одна успешно
прочитанная и проверенная таблица процессов задаёт и liveness, и RSS. При
отказе мониторинга команда останавливается с ошибкой; cleanup использует
последние известные PID и исходную process group. `INT`/`TERM` запускают
существующий десятисекундный grace period с последующим `KILL`, даже если
ребёнок игнорирует `TERM`. Пределы 1 GiB и 15 минут не менялись.
Шесть [поведенческих тестов](../tools/blobray/scripts/tests/test_run_limited.py) запускают
маленькие процессы и проверяют ошибки `ps`, некорректный/пустой вывод,
отмену и сохранение exit status; тяжёлый анализ для этого не запускается.
При полностью недоступной таблице процессов watchdog может завершить только
известные PID и исходную process group. Строгое владение всеми потомками,
включая самостоятельно отделившиеся процессы, обеспечивает backend systemd.

## Текстовые сопоставления, которые нужны

Шаблоны ABI в `audit-source-only.sh` проверяют символы скомпилированного
rlib/ELF, затем decoded-call audit проверяет actual direct targets конечного
образа. Последний анализ запускается через `run-limited` с явно собранным
ESP32-S31 host binary. `open-radio-net` разбирает вывод `iw`/`wpa_cli`; `run-limited` —
таблицу процессов. Проверки формата числовых аргументов и путей относятся
к входным данным. Это не эвристики по Rust source spelling.

Все 11 scripts проходят syntax check соответствующим `bash -n`/`sh -n` и
ShellCheck 0.11.0. Для проверки использован официальный standalone binary
вне репозитория; системная установка не менялась. В installer пустой CDPATH
записан явно как `CDPATH=''`, без изменения поведения. Единственное локальное
подавление SC2329 относится к callback из `trap`: его фактический вызов
проверяется тестом завершения процесса. Installer и команды изменения
состояния HIL при аудите не запускались.

## Итоговая проверка

- Обычный параллельный `cargo test --workspace --locked --offline`: 3940
  passed, 23 ignored, 0 failed. После исправления lock ownership выполнены
  повторные полные прогоны; последний включает явный `AccessLock::file()`.
- `tools/repo/audit-source-only.sh`: PASS, включая 22 Python tests, девять
  изолированных network graphs, строгий workspace Clippy, safety/architecture,
  PAC/qualification и конечный ELF через resource launcher.
- Отдельный network gate: девять compile profiles проходят. Настоящий
  standalone Blobray extraction/build проходит. ShellCheck и shell syntax:
  все 11 entrypoints проходят.
- 90 исходных Cargo manifests/lockfiles побайтно сохранены относительно
  начала текущего этапа. Более ранние структурные перемещения в рабочем
  дереве не отменялись. Форматирование Rust и `git diff --check` проходят.

Стенд не перепрошивался, HIL-сценарии с аппаратурой не выполнялись.
