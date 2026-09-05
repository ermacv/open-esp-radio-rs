# Перенос repository automation в cargo xtask

Дата: 2026-09-05. Пользователь утвердил перенос и уточнил границу: убрать из
скриптов логику, которая принадлежит repository tooling. Подтверждение работы
на иных системах, кроме текущей Linux-среды, не является целью. Перенос
реализован; заменённые девять shell entrypoints и четыре Python-файла удалены.

## Область и владельцы

`oer-xtask` — обычный Rust package в `tools/repo`; root Cargo alias предоставляет
`cargo xtask`. Он организует Cargo metadata, feature consumers, compiler checks,
сборки и проверки реальных артефактов. Policies остаются отдельными модулями
своих проверок, а не общим интерпретатором команд.

`cargo hil`, `cargo blobray`, `cargo memory` и `cargo qualification` сохраняют
свои CLI и ответственность. Xtask вызывает владельцев, не копируя image
builders, evidence schemas, validators или driver behavior. Linux/OpenWrt
стенд, его привилегированный helper/installer и remote shell operations
остаются в HIL. Их перенос на Rust и переработка HIL OS backends в эту работу
не входят.

| Прежний entrypoint | Новый владелец / команда |
| --- | --- |
| `tools/repo/audit-cargo-metadata.sh` | `cargo xtask check metadata` |
| `tools/repo/audit-driver-architecture.sh` | `cargo xtask check architecture` |
| `tools/repo/audit-driver-safety.sh` | `cargo xtask check safety` |
| `tools/repo/check-network-adapter-boundaries.sh` и adjacent Python checker | `cargo xtask check network`, включая `--dependencies-only` |
| `tools/repo/check-esp32s31-examples.sh` | `cargo xtask check examples` |
| `tools/repo/audit-source-only.sh` | `cargo xtask check source-only` |
| `tools/blobray/scripts/check-standalone` | `cargo xtask check blobray-standalone` |
| `verification/vendor/projects/esp32s31/build-analysis-inputs` | `cargo xtask build vendor-probes --chip esp32s31` |
| `tools/blobray/scripts/run-limited` | Отдельный `blobray-run` binary внутри Blobray |
| Repository/limiter Python regression suites и vendor shell-stub tests | Rust tests соответствующих владельцев |
| `hil/host/linux-net/{install.sh,open-radio-net}` и HIL remote snippets | Сохранены: Linux/OpenWrt fixture logic |

Blobray limiter остаётся автономным от xtask: ограничение настоящего анализа
принадлежит инструменту, который должен работать после извлечения из этого
репозитория. Его Linux process/session/cgroup backend сохраняет область
действия, таймауты, отмену, exit status и очистку потомков. Поддержка Windows Job
Objects или macOS не добавляется и не является критерием приёмки.

## Контракты, которые перенос сохраняет

- Восемь независимых Cargo workspace roots; locked resolution исходных
  workspaces и pinned dependencies из исходного lock catalog для временных
  consumers. Cargo config определяется cwd, а не одним `--manifest-path`.
- Разрешение отдельных feature consumers, normal/build dependency closure,
  declared dependency boundaries, compiler-enforced unsafe policy и reviewed
  PAC consumer allowlists. Regex по Rust identifiers не заменяет эти проверки.
- Обычный параллелизм Cargo и Rust tests. Vendor build допускает только явный
  `OPEN_RADIO_ANALYSIS_BUILD_JOBS`; последовательный запуск отдельных build
  profiles не выдаётся за исправление гонок или сбоев.
- Source inventory содержит tracked и nonignored новые файлы, учитывает
  unstaged moves и исключает private/build inputs. Реальные root workspace
  members под `driver/` не исчезают из audit при попадании в `.gitignore`.
- ELF/archive и symbol inspection применяются к скомпилированным артефактам.
  Native objects читает Rust `object`, Rust names — `rustc-demangle`; LLVM
  bitcode требует `llvm-nm` из `llvm-tools` выбранного Rust toolchain. Неизвестный
  формат не становится успешным пустым результатом.
- Сборка, обычный xtask и source gate не выполняют sudo, установку HIL,
  flashing или изменения сети. Private `_oracles`, local run specs и generated
  vendor bundles не становятся source inputs.

## Standalone и vendor probes

Standalone check копирует только Blobray source в временный workspace,
проверяет canonical containment каждого Cargo path dependency и запускает
`cargo check --workspace --all-targets --locked`. Копия имеет собственные cwd и
`target`; root `.cargo/config` не используется. При отсутствии caller override
выбирается channel из root `rust-toolchain.toml`; новый независимый pin не
вводится. Создание отдельного lockfile сохраняет прежний extraction contract.
Все binary targets, включая launcher, входят в проверку автоматически.

Vendor builder хранит три пары role/package/output-directory в одном месте.
`--list-roles` выводит декларацию без сборки; это не доказательство свежести
ELF, авторизации binary input или успешной vendor comparison. Регрессия
сопоставляет роли с реальным project's verification add-on. Невалидный job
limit отклоняется до первого Cargo invocation; ошибка одного artifact
останавливает дальнейшие сборки.

## Проверки и завершение

1. Сопоставить прежние и новые policies, команды и negative verdicts. Перенести
   meaningful Python/shell-fixture cases в Rust и выполнять реальные временные
   Cargo graph fixtures без изменения production sources.
2. Проверить child-process lifecycle и ограниченный launcher настоящими Rust
   process fixtures: argv boundaries, completion, cancellation, memory/runtime
   policy и отсутствие оставленных потомков.
3. Выполнить focused tests и Clippy, затем обычные workspace проверки и новый
   полный source-only gate. Не скрывать ошибки установкой jobs=1.
4. Выполнить standalone extraction со всеми targets и vendor role-contract
   check. Сборки vendor probes дают отдельное build evidence, не результат
   `--list-roles`.
5. Обновить активные README/AGENTS/CLI recipes. Исторические отчёты сохраняют
   исходные команды и результаты с указанием текущей замены.
6. После подтверждения покрытия удалить заменённые scripts/Python files;
   оставшиеся HIL scripts — осознанная граница, а не незавершённая portability
   задача.

Текущие команды и ownership перечислены в
[tools/repo/README.md](../../tools/repo/README.md). Итоговый отчёт должен
отдельно называть фактически выполненные проверки, сбои и ограничения; этот
план сам по себе не является evidence их прохождения.

## Выполненная проверка, 2026-09-05

- Новый `cargo xtask check source-only` — PASS: восемь workspace roots,
  девять isolated network graphs, примеры, strict workspace Clippy, policies
  44 production packages, source-only PAC publication, PHY symbols и
  direct-target audit performance ELF через ограниченный Blobray launcher.
- `cargo test --locked --offline --workspace` — PASS. Отдельные Rust regressions
  покрывают прежние Python cases, некорректные графы, пути с пробелами/Unicode,
  Git worktree, отмену процессов, stdin/pipe handling и выход дочернего
  supervisor до завершения его потомков.
- `cargo xtask check blobray-standalone` — PASS на извлечённом workspace,
  включая launcher. Сборка всех трёх vendor probes — PASS.
- Launcher проверен настоящими процессами: memory/runtime termination,
  сохранение argv/status и отмена; отдельный native systemd test проверил
  реально применённые cgroup limits. Это не подтверждение других host OS.
- Форматирование всех восьми workspace roots и `git diff --check` — PASS.
  HIL Linux helper/installer побайтно совпадают с началом этого этапа.

Реальная сборка vendor probes выявила два дефекта: устаревший `block_on`
вокруг синхронного TX service и старый absolute linker path из закешированного
build script после переноса каталога. Первый исправлен удалением лишней
оболочки; три build scripts теперь получают manifest directory при запуске.
Регрессия компилирует настоящий build script один раз, затем запускает его
с перемещённым каталогом. Очистка Cargo cache или ограничение параллелизма
не использовались как исправление.

Повторное lifecycle review выявило, что обычный короткий shutdown grace
xtask мог прервать очистку отдельной Blobray session. Вызов launcher теперь
имеет собственные 20 секунд на завершение; это не ограничивает ресурсы или
параллелизм Cargo. Noninteractive capture закрывает stdin и одновременно
читает оба output pipes; cleanup выполняется перед ожиданием читателей.

Локальные журналы этого этапа: `~/.cache/oer-xtask-migration/`. Они не содержат
vendor inputs и не добавляются в source tree. Этот результат подтверждает
сборки и проверки исходников; новые аппаратные HIL сеансы не выполнялись.
