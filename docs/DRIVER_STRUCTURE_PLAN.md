# План структурирования driver

Аудит: 2026-09-04; реализация обновлена 2026-09-05. Основание: [глобальный аудит](DRIVER_STRUCTURE_AUDIT.md).
Реализация начата. Production behavior, набор поддержанных режимов и
аппаратная qualification не расширяются.

Первая волна (2026-09-04), сохранённая в текущем worktree:

- Вынос 347 test/helper modules в отдельные дочерние файлы; compiled discovery
  после extraction сохранил все 3891 записи root workspace.
- Перенос 16 crate containers с сохранением package identities и dependency
  revisions; owned/compat resolved graphs сохранены после нормализации путей.
- HCI `command`, Embassy `monitor`, MAC `cold`, SoC `cache/flash/dma` и PHY
  namespaces; local STA child execution вынесен из supervisor.
- S31 ESF alignment перенесён из portable IEEE80211 в chip Wi-Fi DMA.
- Handwritten raw-PAC sidecars получили собственные deny lints и scoped
  unsafe exceptions; generated outputs не редактировались.
- README/UNSAFE приведены к текущим каталогам и границам владения.

Вторая волна (2026-09-05):

- HAL: 27 leaf modules сгруппированы по доменам, affine owners вынесены в
  `owner.rs`; корневой `lib.rs` уменьшен с 1078 до 178 строк.
- Restricted PAC: 63 handwritten leaf modules сгруппированы, корневое владение
  выделено в `ownership.rs`; `lib.rs` уменьшен с 2049 до 277 строк. Прежние
  закрытые операции не стали публичными; generated outputs не менялись.
- Bluetooth Embassy: `controller/{owner,dispatch,reset,response}` и
  `session/{dtm,advertising,scan,peripheral}`. Async dispatch сохранён целиком,
  без новых await boundaries и без изменения возвращаемых владельцев.
- Chip Bluetooth: `controller/{boot,hal,time}`, `interrupt` и `scheduler`.
  Feature-gated scheduler core отделён от доступных host-коду leaf modules.
- Wi-Fi MAC: 12 модулей сгруппированы в `rx`, `tx`, `rate`; прежние публичные
  module aliases сохранены. 86 внешних MAC-тестов разделены на десять групп
  с общей test-only поддержкой, один Cargo test target сохранён.
- Radio: переносимые `wifi` contracts отделены от `runtime::embassy`, старые
  root exports сохранены. Optional Embassy tests теперь используют общий
  synthetic profile вместо ошибочной ссылки на chip dependency; проверены
  ownership/cancellation/stop contracts в ранее не собиравшемся test profile.
- Portable `DataEncapPlan` хранит `WmmAccessCategory`. Кодирование очередей,
  packet type, descriptor priority и callback routing находится в chip
  `chips/esp32s31/ieee80211/mac/src/tx/metadata.rs`; все repository consumers обновлены.
- Активные vendor bindings обновлены под фактические compiled namespaces,
  включая PHY bindings предыдущей волны. Проверена неизменность остальных
  disposition fields; MATCH/DIFF/INCOMPLETE policy сохранена. Это не новая
  vendor/HIL qualification.
- Metadata audit проверяет non-ignored additions до staging и явно исключает
  `_oracles`, включая force-added inputs. Два regression tests вошли в
  source-only pipeline; все 80 manifests охвачены восемью workspace roots.

Третья волна (2026-09-05):

- HCI transport/bootstrap/controller разделены по хранению пакетов, очередям,
  декодированию, bootstrap state и joint controller owner. 98 tests и public
  root exports сохранены; проверены 472 прежних тела функций.
- SoC mem2mem разделён на descriptor/registers/transfer/completion. 38 тел и
  81 декларация сохранены; дополнительные private borrows остаются внутри
  прежнего mem2mem ownership domain. CACHE coordinator не добавлялся.
- Wi-Fi ISR bindings, три static owners и создание IRQ epoch вынесены в
  integration `interrupts.rs`. Handler bodies, IRAM attributes и маршрутизация
  через один interrupt runtime сохранены.
- RX frontier/state/lifecycle, abstract delay и существующий storage profile
  перенесены в chip Wi-Fi. Embassy Timer и staged endpoints остались в adapter.
  Шесть owner tests перенесены; `require_reset` стал узким публичным методом
  terminal quarantine и не обещает quiescence. Новых Send/Sync/owner copies нет.
- STA capabilities и S31 channel selection находятся в chip STA `profile`;
  portable `AssociationRequest::encode` получает явный профиль. Пять profile
  tests перенесены в chip; новый portable test проверяет явную передачу профиля.
- HT AP capabilities находятся в chip AP `profile`; семь portable encoder
  functions получают `HtLocalCapabilities`. Геометрия канала очищает/задаёт
  принадлежащие ей width/SGI bits, включая произвольные новые входные profiles.
  Это не меняет прежний профиль `0x100c/0x03/0xff/0x01`.
- Authentication/association retry state и deadline policy перенесены из
  IEEE80211 в `ieee80211/sta/join/{authentication,association}`, вместе с семью
  тестами. Deadline 1000 ms, auth limit 3 и association interval 160 ms сохранены.
- Portable AP `limits` владеет прежним software budget 15 клиентов/TIM 2 bytes
  и validated limit types. Новый chip compile-time guard доказывает, что
  admission ceiling не превышает число pairwise key slots. Это не утверждение
  о существовании отдельного runtime comparison. Variable-capacity API не введён.
- Secret owner теперь называется `CcmpKey`, старый `AlignedCcmpKey` оставлен
  alias. Zeroization, private bytes и repr(C, align(4)) сохранены. Аудит цепочки
  key → MAC → HAL → PAC подтвердил borrowed bytes и `chunks_exact`/`from_le_bytes`,
  без casts/DMA publication; дополнительный upload-owner/copy не нужен.

Четвёртая волна (2026-09-05):

- Синхронный staged RX algorithm выделен в chip `rx/transaction` внутри
  существующего Wi-Fi crate. `service` временно заимствует live ring, static
  storage и staging pool; persistent owners и их поля не перемещались.
- `Admission` принимает только факты о завершённом unit. `Publisher` получает
  единственный `NetworkRxFrame`; отказ возвращает тот же lease. Прежний
  error path освобождает его на месте и возвращает `Corrupt`, без нового retry.
- Конкретный publisher, STA/AP classification, Embassy timestamp, observer,
  telemetry selector и clock implementations остаются в adapter. Статические
  `Hooks` сохраняют существующие точки измерений и feature gates; optional
  hardware samples не вычисляются при выключенных наблюдениях.
- `Counters` заимствует прежние три поля и обновляет их на прежнем месте,
  включая сохранение обновления при последующей ошибке final frontier read.
  Chip не создаёт второй accumulator, endpoint или physical owner.
- `ready(chip::service(...))` сохраняет исполнение до первого poll. Chip helper
  встраивается в прежний hot adapter wrapper; `forbid(unsafe_code)` chip-crate
  и существующие stack/placement allowances не ослаблены.
- Старые adapter admission/observation imports сохраняются через reexports.
  Новые канонические имена короче внутри `rx::transaction`; package identities,
  dependencies, features и lifecycle state machines не менялись.
- Три regression tests проверяют eager publication, rejection/reclaim исходного
  буфера и удержание accepted lease; проверка `Busy` доказывает возврат полного
  владельца при отказе stop. Positive/compile-fail примеры закрепляют передачу
  affine frame через новый публичный порт.
- HIL README приведён к существующему stack policy: unreviewed frames >8 KiB
  отклоняются, hard ceiling 50 KiB, move ceiling 4 KiB. Это исправление описания,
  а не увеличение разрешённого размера.

Проверки четвёртой волны:

- Workspace check, fmt и Clippy проходят; warning inventory совпадает с третьей
  волной: 73 прежних Bluetooth diagnostics, ни одного нового. Изменённые chip
  Wi-Fi и adapter проходят focused strict Clippy без warnings.
- Полный повторный workspace test: 3879 passed, 22 ignored, 0 failed. Все
  прежние 3896 discovery entries сохранены, добавлены три regression tests и
  два doctest contracts; итог 3901. Первый прогон остановился на неизменённом
  HIL fixture-lock test, затем его отдельный запуск и полный повтор прошли.
  Lock implementation и тест не менялись.
- Architecture: 44 production crates / 131 isolated feature profiles; safety:
  30 safe / 13 audited unsafe packages; metadata: 80 manifests / 8 roots;
  qualification manifest VALID. Все эти проверки проходят.
- Adapter default, diagnostics и all-features host tests проходят. All-features
  host profile сохраняет шесть прежних target-only dead-code warnings;
  default strict Clippy чист. Rustdoc двух затронутых packages проходит с
  `-D warnings`; четыре прежние неразрешимые ссылки исправлены явно.
- Независимое сравнение production algorithm: 2477 canonical tokens совпадают
  после 55 перечисленных context/name/observation substitutions. Сохранены
  порядок 53 физических вызовов, loops, early errors, counters и progress
  precedence. Concrete cfg/hooks проверены отдельно; это не доказательство
  бинарной тождественности или on-air performance.
- PAC publisher `--check` проходит. Оба generated Rust outputs и все восемь
  lockfiles побайтно совпадают с началом волны.
- Собраны `performance`, `correctness`, `diagnostic-core0-rx-coarse`,
  `diagnostic-core0-rx-cycles` и `diagnostic-task-poll`. Каждый образ проходит
  placement, stack-frame и autonomous source-graph gates. Отдельного chip
  helper в performance ELF нет: код встроен в прежний hot adapter wrapper.
  Два RX stack frames performance выросли с 640/656 до 784/784 bytes;
  diagnostic RX frames занимают 784–1152 bytes. Допуски не расширялись.
  Это проверка сборки/размещения, без прошивки и on-air HIL.
- Финальный performance application SHA-256:
  `7b14927c345bf2322e32c9ac9ecf8d3e6bcbc9fb80c9b59aa91723a4681ed39a`.
  Ограниченный direct-target audit: 0 forbidden / 840625 просмотренных instructions,
  3422 unsupported. Проверены только статически разрешимые переходы; это не
  доказательство всех косвенных переходов или смысла unsupported instructions.
- Полный source-only снова остановлен прежним strict Bluetooth Clippy
  (52 ошибки lib-test). Его последующие gates этим запуском не выполнены;
  перечисленные architecture/safety и image checks выполняются отдельно.

Пятая волна (2026-09-05):

- Канонические семейства каталогов — `ieee80211`, `ieee802154`, `bluetooth`.
  Перенесены 13 контейнеров с 17 crates; WPA2 находится в
  `ieee80211/security/wpa2`, переносимый LE LL — в `bluetooth/le/ll`.
  Generic/chip/Embassy/esp-hal/integration пути используют одну конвенцию.
- Cargo package identities и Rust API сохранены. Production Rust побайтно
  совпадает с началом волны; ни owners, ни await boundaries не менялись.
  Shared PHY, HCI, аппаратные PAC names и прикладной Wi-Fi facade сохраняют
  свои границы. Внутренние protocol/extension modules — следующий этап.
- Обновлены manifests, source bindings qualification, stack source suffixes,
  архитектурные проверки и документация. Исторические vendor/HIL evidence
  сохраняют исходные пути. [Карта контейнеров](driver-audit/container-moves.csv)
  дополняет предыдущие карты; [текущее дерево](driver-audit/current-tree.txt)
  показывает окончательное размещение этой волны.

Проверки пятой волны:

- Root workspace: `check`, `fmt`, tests и обычный Clippy проходят;
  3879 passed / 22 ignored / 0 failed. Compiled inventory — прежние 3901 записи
  после нормализации только путей doctests. Диагностика strict source-only
  совпадает с предыдущей волной: скрипт останавливается на Bluetooth Clippy
  (52 ошибки lib-test); полный source-only PASS не заявляется.
- Root/owned/compat resolved graphs идентичны после нормализации путей
  (470/185/184 packages). Все 80 manifests и 250 path dependencies сохраняют
  значения и направления. Metadata audit проверил восемь workspace roots;
  network adapter boundary check проходит для owned/compat.
- Architecture: 44 production crates / 131 isolated feature profiles; safety:
  30 safe / 13 audited unsafe packages. Qualification manifest schema 4 VALID;
  это проверка спецификации, без новой hardware evidence.
- Независимый review: 1156 driver Rust files побайтно сохранены; из 1770 Rust
  во всём снимке отличаются только две test fixture строки пути в memory-report.
  Все восемь lockfiles и оба generated PAC outputs сохранены; publisher
  `--check` сообщает 0 written / 5 verified. Build/linker/placement inputs
  сохранены, кроме 25 source suffixes в stack policy; лимиты не менялись.
- Собраны пять образов: `performance`, `correctness`,
  `diagnostic-core0-rx-coarse`, `diagnostic-core0-rx-cycles`, `diagnostic-task-poll`.
  Все проходят placement, stack-frame и autonomous source-graph checks.
  Все 79 проверенных локальных Markdown links разрешаются; актуальное дерево
  охватывает 1213 driver files. Финальный diff не содержит whitespace errors.
- Performance RX stack frames остались 784/784 bytes. Performance application
  SHA-256: `fb71adc7fbd7ec5969a7e37490659161cad1f437cd0079e0d41defdcef6c4515`.
  Хеш образа изменился: тождество исходников после переноса не объявляется
  бинарной идентичностью. Ограниченный direct-target audit проходит:
  0 forbidden / 840625 instructions, 3422 unsupported; он охватывает только
  статически разрешимые переходы. Прошивка и on-air HIL не выполнялись.

Независимые сравнения настоящих старых и новых production функций дали MATCH:
5760 STA association encodings/errors/output-buffer states, 108 channel
selections и 32928 HT/beacon/AP association cases. Текущий RX descriptor budget
равен 96; старое описание HT-профиля через 64 descriptors уточнено как provenance,
без изменения advertisement или live storage geometry.

Карта третьей волны: [responsibility-moves.csv](driver-audit/responsibility-moves.csv).

[Карта переносов второй волны](driver-audit/namespace-moves.csv) и
[актуальное дерево](driver-audit/current-tree.txt) содержат новые пути.
Crate count остаётся 44; package names, dependency revisions и lockfiles
в этой волне не менялись.

Независимое сравнение настоящих старых и новых функций TX metadata дало MATCH
для 8192 комбинаций UP/role/group/QoS/no-ack/HE-Control, всех 256 priority
values и 131072 role/EtherType callback routes. Новых frame copies и
allocations нет. Механическое сравнение handwritten тел выполнено отдельно
для HAL/PAC, Bluetooth, MAC и radio с учётом namespace/visibility substitutions.

Описанные ниже более широкие разделения capability profiles, chip runtime
contracts, package renaming и возможное слияние crates остаются отдельными
этапами. Ниже сохранён baseline проверок третьей волны, до выделения
`rx/transaction`:

| Проверка | Результат |
|---|---|
| `cargo fmt --all -- --check`, отдельный Wi-Fi integration fmt | PASS |
| `cargo check --workspace --locked --offline` | PASS |
| `cargo test --workspace --locked --offline` | 3874 passed, 22 ignored, 0 failed |
| Compiled test inventory | Все 3894 записи второй волны сохранены с учётом переносов; добавлены 2 tests, итог 3896 |
| `cargo clippy --workspace --all-targets --locked --offline` | PASS с существующими Bluetooth warnings |
| Radio `--all-features` tests | PASS: 22 tests, включая optional Embassy ownership contracts |
| SoC target feature checks | PASS: `axi-gdma-mem2mem` и `psram-dma-diagnostic` |
| HAL/PAC focused validation profile | PASS: 214 unit tests + 21 doctests; host/target checks и strict Clippy без warnings |
| Metadata audit + discovery regression | PASS: 8 workspace roots; 2 regression tests |
| Architecture audit | PASS: 44 production crates, 131 isolated feature profiles |
| Safety audit | PASS: 30 safe + 13 audited handwritten packages, raw backend проверен отдельно |
| Target release checks примеров, включая compat station | PASS |
| PAC `project publish --check` | PASS: register validation и четыре output, 0 written |
| Qualification manifest validation | VALID schema 4; новые readiness/HIL claims не заявляются |
| Performance HIL image build | PASS: placement, stack-frame и autonomous source graph |
| Ограниченный direct-target audit образа | PASS: 0 statically resolved forbidden radio-ROM targets; отмечена 3421 unsupported instruction |
| Полный `audit-source-only.sh` | Остановлен strict Clippy `-D warnings` на существующем Bluetooth-коде |

Причины последней остановки не внесены переносом: unused/dead-code diagnostics
совпадают с baseline. Все 73 диагностических блока Clippy совпали после
нормализации namespace и пути scheduler/core: 60 dead-code, 8 large-error,
4 large-enum и 1 unused-import; новых предупреждений нет. Последняя сводка
strict Clippy — 52 ошибки в Bluetooth lib-test. Проверки не ослаблялись.
Архитектурный и safety-аудиты, сборка образа
и проверка его direct targets выполнены отдельно. Это не полный PASS
source-only pipeline; direct-target audit ограничен статически разрешаемыми
переходами и не доказывает отсутствие всех косвенных обращений.

Образ третьей волны был собран в
`target/hil/esp32s31/psram-code-psram-data-psram-stack-performance/`;
application SHA-256 `647bcc2bcf01ede9edca85671be6c4f554f794a343c165d9d186936c1eb3fde2`.
Плата не прошивалась; on-air HIL не выполнялся. Generated Rust и lockfiles
побайтно сохранены; declared dependencies и resolved owned/compat feature graphs
совпадают с baseline после нормализации путей.

Изменение публичной навигации: старые PHY module paths `phy_*` заменены
вложенными пространствами; root type/value reexports сохранены. Все найденные
consumers в driver, examples, HIL и verification переведены. Внешние consumers
старых module paths должны перейти на новые namespaces.
Также изменён portable API `data`: поля `queue_class`/`packet_type` заменены
на `access_category`; аппаратные значения доступны через chip
`tx::metadata::DataTxMetadata::from_encapsulation`. Helpers `queue_class`,
`descriptor_priority_byte`, `completion_callback_mask` принадлежат тому же
chip-модулю. Остальные затронутые HAL/PAC/MAC/radio root exports сохранены.

Публичные API третьей волны:

- `AssociationRequest::encode(output, &AssociationCapabilities)` требует
  явного профиля; production caller передаёт chip `profile::ASSOCIATION_CAPABILITIES`.
- `select_sta_association` и hardware selection из IEEE80211 заменены chip
  `wifi_sta::profile::{select_association, Selection}`.
- HT/beacon/AP association encoders принимают первым аргументом
  `HtLocalCapabilities`; chip AP использует `profile::HT_CAPABILITIES`.
- Authentication/association Attempt/Event/Error/Runtime types и retry schedule
  находятся в portable `wifi_sta::join::{authentication,association}`.
  Обратных aliases в IEEE80211 нет: они создали бы dependency cycle.
- HCI, SoC, RX adapter, AP limits и key owner сохраняют прежние public imports
  через reexports/alias; package identities и dependencies не меняются.

Staged RX transaction выделен без chip → adapter dependency. Точный owner
mapping, typed rejection и границы диагностики записаны в
[RX ownership contract](../driver/adapters/embassy/esp32s31/ieee80211/src/datapath/rx/README.md).
Production storage/placement по-прежнему выбирает integration; defaults и
executor-owned endpoints не объединялись в новый runtime crate.

Независимый аудит обнаружил отдельный прежний lifecycle defect: отмена
`Esp32s31StagedRxEpoch::start` на pending settle delay может оставить `Vacant`
и потерять recoverable prepared owner. Walker в этот момент ещё не включён;
это не доказательство live-DMA use-after-free. Исправление должно сохранять
`Prepared` в epoch на время borrowed delay и отдельно проверять cancel/retry
с теми же ring/pool/queue. В структурный перенос изменение state machine не
включено; обычные success/error returns сохранены.

Variable-capacity AP API, полный package rename и consolidation остаются
отдельными возможными изменениями. Для текущей читаемости дополнительные
crates или копии hardware/key owners не понадобились.

Работу вести небольшими reviewable изменениями. В одном изменении не смешивать
перемещение файлов, смену package identity, обновление внешней зависимости и
новые ownership transitions. Существующие запрещённые режимы остаются
запрещёнными. Каждый этап заканчивается собираемым состоянием.

## Уточнение границ adapters, common и integration

Снимок повторного аудита после пятой волны, до реализации шестой, 2026-09-05. Проверены
все 18 manifests этих каталогов, их потребители, исходные модули и ключевые
ownership transitions. Три независимых review покрыли adapters, common и
integration. Это структурный review, не новая проверка всех lifecycle paths.

| Каталог | Crates | Файлы / Rust | Строки Rust, включая тесты | Вывод |
|---|---:|---:|---:|---|
| `adapters` | 14 | 309 / 291 | 91586 | Смешаны внешние bindings, radio runtime, profile policy и сетевой движок |
| `common` | 2 | 12 / 10 | 2352 | Две связные низкоуровневые обязанности; неопределённое имя контейнера |
| `integration` | 2 | 38 / 35 | 14277 | Нужный concrete composition root, но также полный lifecycle runtime |

Числа получены до исправления комментария Drop из `rg --files` и UTF-8
`splitlines()`; ignored build outputs исключены. Размер помогает выбрать
приоритет review, но сам по себе не
доказывает нарушение границы. Предыдущие переносы унифицировали пути;
семантические смешения ниже ими не устранены.

### Common: общее между слоями, а не произвольные helpers

[DMA crate](../driver/common/dma/src/lib.rs) содержит доказательства стабильного
backing, отдельные TX prepare/start authorities, RX/TX leases, возврат индексов
и affine SPSC. Он не зависит от Embassy, PAC, allocator или network stack.
Его используют девять production packages; у
[network values](../driver/common/network/src/lib.rs) — семь. Оба crate без
зависимостей. Network содержит всего 58 строк: endpoint ID, link state,
Ethernet header length и ошибки приёма, без очереди или STA/AP policy.

Фактических direct consumers DMA crate в Bluetooth/IEEE 802.15.4 пока нет;
HAL/PAC используют его в Wi-Fi MAC. Общность здесь означает независимость
от слоя и конкретного network adapter. Она не доказывает универсальность для
всех радиопротоколов и не оправдывает перенос сюда случайных утилит.

Предметные направления: `common/dma → memory`,
`common/network → network/interface`, сохранив два существующих crate.
В memory сначала выделять внутренние `dma/{authority,backing}`, `queue`,
`tx` и `rx`; не создавать crate на каждый helper. `AffineSpscQueue` не требует
DMA, а storage пока имеет сетевые и diagnostic особенности: это не обещание
универсального allocator. 58 строк network values делить дальше незачем.

Пересечение с adapters/integration обосновано: memory определяет безопасный
lease; adapter связывает его с очередью возврата; integration размещает один
пул. Ни proof type, ни callback не должны получать второй hardware owner.
В [ReturningStableDmaBacking](../driver/common/dma/src/pinned_tx.rs) исправлено
описание Drop: он возвращает backing/index после прекращения аппаратного
доступа, но сам не останавливает DMA и не доказывает completion. Код не менялся.

### Adapters: реальные мосты и обязанности, скрытые их именем

| Текущее размещение | Реальное содержание | Решение для границы |
|---|---|---|
| `network/embassy/{owned,compat}` | Foreign network API, packet handoff, bounded queues, tokens | Сохранить два разных adapters; группировать в network domain |
| `embassy/esp32s31/ieee80211-compat` | Copied-frame endpoint → selected radio burst | Сохранить отдельный bridge; не смешивать с owned path |
| `esp-hal/esp32s31/{ieee80211,ieee802154}` | Upstream witnesses, IRQ routing, реализации hardware ports | Обоснованные adapters, включая удержание IRQ owner |
| `esp-hal/esp32s31/radio` | Singleton reservation, platform claims и IRQ bindings | Явный coordinator; ещё не общий работающий Wi-Fi/Bluetooth lifecycle |
| `esp-hal/esp32s31/soc` | Cache/MMU/GDMA backend, transfer owner и completion | Предметный SoC backend; сохранять отличие upstream accessors от собственного PAC |
| `embassy/esp32s31/runtime` | Embassy executor/time ABI через software interrupt и timer | Честный внешний backend; обозначить executor/time, отличать от radio runner |
| `embassy/esp32s31/{bluetooth,coex,ieee802154}` | Исполнение chip owners через waits/channels, stop/quarantine | Явно обозначить radio execution; размер actor не означает, что его state нужно дробить |
| `embassy/ieee80211` | Mailboxes, association/network handoff, task shutdown, poll boundary | Разделять contracts и execution модулями; не переносить всё в common |
| `embassy/esp32s31/ieee80211` | Role execution, datapath, security transaction, resource profile, telemetry | Приоритетное выделение chip transactions и product profile |
| `network/research` | Собственные Ethernet/ARP/IPv4/ICMP/UDP processing и frame construction | Экспериментальный network engine; слово adapter неверно |

Критерий adapter — какой внешний контракт он связывает с каким внутренним
портом. Очередь, token state или владение IRQ capability могут быть необходимы
для этой связи. Требование «adapter всегда stateless и тонкий» здесь неверно.
Основание принципа — [оригинальная Ports and Adapters architecture](https://alistair.cockburn.us/hexagonal-architecture/);
конкретная классификация выше получена из кода этого репозитория.

Наиболее существенные находки:

1. [ConnectedWpa2Security](../driver/adapters/embassy/esp32s31/ieee80211/src/roles/station/control.rs)
   хранит supplicant, GTK slot/material и replay endpoint. `process_group_message1`
   выполняет S31 key-generation admission, rotation, rollback и quarantine.
   Это chip STA security transaction. Выделять целиком существующего owner
   в chip STA `connected/security`, оставив Embassy доставку/ожидания снаружи.
   Минимальное value-сообщение `ConnectedSecurityFrame` нужно перенести либо
   преобразовать на границе; нижний crate не должен зависеть от adapter.
   WPA2 dependency в chip STA уже есть. Не разрывать совместное владение
   key/replay/supplicant и не менять ограничения смены GTK при переносе.
2. [Adapter composition/resources](../driver/adapters/embassy/esp32s31/ieee80211/src/composition/resources.rs)
   прямо называет свои значения integration policy. Здесь выбираются software
   TX horizon и Xarxa packet pools, а сами пулы размещает integration
   [radio_resources](../driver/integration/esp32s31/embassy/ieee80211/src/radio_resources.rs).
   Выбор значений должен быть в production profile рядом с окончательной
   композицией; reusable storage types могут оставаться ниже. Сохранить точные
   значения, static claims и memory geometry. Перенос defaults вверх с
   обратными reexports из adapter создаст цикл; consumers нужно обновить,
   а не копировать constants в два слоя.
3. AP [network_tx](../driver/adapters/embassy/esp32s31/ieee80211/src/roles/access_point/network_tx.rs)
   владеет lease arena, per-flow selection, standby aggregate, power-save и
   DTIM-authorized release. Это AP execution, а не общий network adapter.
   Сначала организовать `tx/{queue,power_save,aggregate,completion}` внутри
   существующего owner domain. `datapath/software_tx_queue` имеет единственного
   production consumer — этот AP модуль; отправлять его в common оснований нет.
4. [Research engine](../driver/adapters/network/research/src/engine.rs) сам
   разбирает и строит сетевые пакеты. `physical.rs` отдельно связывает его с
   pinned DMA batch. В текущем repository engine crate подключён только как
   dev-dependency конкретного Embassy Wi-Fi crate; consumer находится в
   `roles/station/tx/tests.rs`. Подключённая shipping alternative integration
   не обнаружена. До появления product consumer учитывать его как эксперимент,
   не как рабочий альтернативный stack. Убрать из категории adapters;
   статус отдельного research implementation определить до выбора постоянного
   места. Engine/materializer можно разделить модулями без нового crate.

Нынешняя иерархия смешивает оси: `adapters/embassy/...` группирует по библиотеке,
а `adapters/network/embassy/...` — сначала по функции. Для network domain
целевые понятия — `interface`, `adapters/embassy/{owned,compat}`, engine.
Для radio execution — явно названный `runtime/embassy` после выделения chip
transactions. Это направления, а не разрешение механически переместить все
Embassy crates: executor/time backend и PHY time bindings остаются adapters.
`esp-hal/.../soc` не следует смешивать с generated PAC или складывать в
новый неопределённый `support/platform`.

### Integration: финальная композиция плюс исполнение всего радио

Wi-Fi integration содержит 10648 строк Rust, из них 7857 находятся в
`supervisor`; Bluetooth — 3629 строк. В обоих случаях это связный concrete
radio lifecycle, но обещание «только собрать объекты» не описывает содержание.
Просто назвать весь каталог `composition` было бы ещё одной косметической
заменой. Обоснованы final IRQ bindings, static allocation, one-time claims,
выбор owned/compat, facade и корневое удержание failed owners.

Есть следующие конкретные смешения:

- В Wi-Fi [supervisor/access_point](../driver/integration/esp32s31/embassy/ieee80211/src/supervisor/access_point.rs)
  находятся общий `ProductionRxRing`, `ProductionWifiPhysicalResources` и
  даже `ProductionStationRoleResources`. Их область — общий physical owner
  и передача между ролями. Следующее безопасное выделение — внутренние
  `supervisor/physical` и role-transition modules с прежними полями/методами.
- [composition/start](../driver/integration/esp32s31/embassy/ieee80211/src/composition/start.rs)
  выполняет generic cold PHY/MAC startup через chip API и `PhyAsyncDelay`.
  Это кандидат в существующий chip startup; выбор конкретных delay/observer
  и production profile остаётся наверху.
- Bluetooth [phy_time](../driver/integration/esp32s31/embassy/bluetooth/src/phy_time.rs)
  — Embassy binding к shared PHY, как Wi-Fi
  [composition/phy](../driver/adapters/embassy/esp32s31/ieee80211/src/composition/phy.rs).
  Но Bluetooth проверяет 1 MHz/overflow и fail-stops, а Wi-Fi напрямую вызывает
  Timer. Сгруппировать одну ответственность можно, объединить эти реализации
  без отдельного изменения контракта нельзя. Shared PHY не принадлежит Wi-Fi.
- Bluetooth [system](../driver/integration/esp32s31/embassy/bluetooth/src/system.rs)
  содержит приоритет IRQ faults, command/modem scheduling и удержание
  quarantine. Группировать `runner/policy/quarantine` отдельно от construction
  и memory claims, сохраняя единый корневой owner.

Повторение слова resources обычно означает разные уровни: тип → выбранная
ёмкость → static allocation → переданный lease. Это не четыре владельца одной
памяти. Аналогично две esp-hal platform compositions сейчас требуют одни и те
же upstream singleton witnesses и взаимоисключают друг друга. Общий radio
coordinator остаётся отдельным lifecycle проектом, не исправлением пути.

Есть конкретное ограничение DAG: `radio` зависит от generic Embassy adapter,
а integration реализует `radio::EmbassyWifiRoleEpochRunner`. Перенос всего
supervisor в generic adapter создаст цикл. Перенос в chip-specific adapter
привяжет нижний runtime к facade и окончательному resource profile. Сначала
нужны внутренние modules; нижний port выделять для конкретной операции, а
concrete facade implementation оставлять в integration. Существующий
`Esp32s31StationEnginePort` уже показывает такое разделение.

### Обновлённый приоритет следующих изменений

1. Внутренне выделить общие physical/role-transition owners из AP supervisor;
   отдельно обозначить construction, storage, runner и diagnostics. Новых
   owners, crates и правил завершения не добавлять.
2. Выделить chip STA security transaction и generic chip startup целиком;
   сохранить await/cancel/error/rollback behavior, cfg и public consumers.
3. Перенести выбор resource profile к окончательной композиции; исключить
   обратные dependencies и второй набор defaults. Сгруппировать PHY time
   adapters, сохранив разные clock/failure contracts.
4. После этого дать предметные пути memory/network и radio runtime, уточнить
   статус research engine. Затем продолжить protocol/state/extensions и
   сокращение имён из терминологического плана.

Каталоги не заменяют Rust privacy: границы доступа задаются module/API
visibility и зависимостями, как описано в
[Rust Reference](https://doc.rust-lang.org/reference/visibility-and-privacy.html).
Для структурных изменений нужны проверки DAG, точных возвратов владельцев,
feature profiles, test inventory и конечного placement/stack. Regex-запрет
слов `common` или `resources` не проверяет архитектуру.

В исходном review каталоги и поведение не менялись. Исправлена только неточная
документация Drop, уточнена source map и дополнен существующий план. Для этой
documentation-only работы проверяются diff, форматирование и локальные
ссылки; результаты предыдущей кодовой волны не выдаются за новый HIL run.

### Шестая волна: физические владельцы, startup и security

Реализована первая часть уточнённого плана, 2026-09-05:

- Общие RX/physical resources из AP supervisor перенесены в `physical.rs`,
  сохранённые состояния станции и split/join — в `role_transition.rs`,
  AP diagnostics — в `access_point_observation.rs`. AP-файл уменьшен с 1786
  до 1032 строк; async lifecycle, parked owners и fault handling остались
  в нём. Внутри supervisor открыты только 15 необходимых scoped доступов
  (10 полей, 2 enum, 3 метода); наружу поля не опубликованы.
- Generic cold PHY/MAC startup перенесён из integration `composition/start`
  в [chip startup](../driver/chips/esp32s31/ieee80211/src/startup.rs).
  Старый integration module оставляет внутренний импорт. Конфигурация,
  возвращённый stopped owner/cache и failure variants сохранены; функция
  доступна только на прежнем target. Выбор конкретного delay остаётся наверху.
- [Chip STA connected/security](../driver/chips/esp32s31/ieee80211/sta/src/connected/security.rs)
  теперь удерживает весь `ConnectedWpa2Security`, включая supplicant, GTK и
  replay. Девять тел методов, поля и error/rollback/await порядок сохранены.
  Adapter сохраняет публичные имена через reexports; mailbox удерживает alias
  классифицированного EAPOL сообщения. Для межкрейтового вызова опубликованы
  enum сообщения и три метода: `tx_in_flight`, `complete_tx`, `process`.
  Это входы для существующего владельца и hardware/TX ports; helpers и поля
  остаются закрытыми. Классификация frame — обязанность RX binding, не новая
  криптографическая проверка в enum.
- Все 24 adapter control tests сохранены по именам. Три вызова прежнего
  private duplicate-M3 helper теперь проверяют тот же путь через
  `process(Unprotected(frame))`; тесты forged M3, rekey, RSC, rollback и
  quarantine остаются с production owner.
- Новых crates/dependencies/features/unsafe/Send/Sync нет. Статические
  allocations, storage geometry и владельцы не менялись. Stack policy меняет
  только function/source selectors перенесённого startup, сохраняя 18432-byte
  limit; AP async selector остаётся на прежней функции.

Проверки шестой волны:

- Workspace `check`, tests и обычный Clippy проходят: 3879 passed / 22 ignored /
  0 failed; compiled inventory совпадает буквально, все 3901 записи сохранены.
  Точечный Clippy трёх затронутых chip/adapter packages с `-D warnings` чист;
  Rustdoc этих packages с `-D warnings` также проходит.
- Независимое сравнение: startup — 3 function bodies / 6 ordered declarations;
  security scope — 101 / 120; весь supervisor — 98 / 262. Поля/варианты/порядок
  операций сохранены с перечисленными namespace и visibility substitutions.
  Из 1770 исходных Rust files 1761 побайтно неизменны, 9 изменены, 6 добавлены.
- Все 80 manifests, 8 lockfiles и 112 PAC Rust files побайтно сохранены.
  Root/owned/compat metadata совпадает без нормализации (470/185/184 packages).
  Architecture — 44 packages / 131 feature profiles; safety — 30 safe /
  13 audited unsafe. Qualification manifest schema 4 VALID; publisher
  `--check` — 0 written / 5 verified. Новая hardware qualification не заявляется.
- Полный source-only останавливается на прежнем Bluetooth strict Clippy
  (52 ошибки lib-test); диагностические сообщения совпадают с пятой волной.
  Последующие architecture/safety/image gates выполнены отдельно.
- Performance application SHA-256:
  `383426aba137b71fbee639f3eada86e21b43372ac34b517806e2d4c82dbc6003`.
  RX stack frames — прежние 784/784 bytes; вынесенный startup frame — 7024 bytes
  при прежнем лимите 18432. Бинарная идентичность не заявляется.
  Ограниченный direct-target audit: 0 forbidden / 840676 instructions,
  3428 unsupported. Проверяются статически разрешимые переходы, а не все
  косвенные вызовы или смысл unsupported instructions.

- Все пять образов — performance, correctness, diagnostic-core0-rx-coarse,
  diagnostic-core0-rx-cycles, diagnostic-task-poll — проходят placement,
  stack-frame и autonomous source-graph gates. Образы не прошивались;
  on-air HIL не выполнялся. Проверены 95 локальных Markdown links и
  актуальное дерево 1219 driver files; fmt и diff checks проходят.

Дальнейшие шаги: product resource profile и PHY time bindings (пункт 3),
затем предметные пути memory/network/radio runtime и protocol modules.
Перенос профиля требует сохранить проверяемый claim contract и tests, обновить
consumers без adapter→integration reexports; существующие defaults нельзя
дублировать ради совместимости.
Read-only подготовка следующего шага нашла шесть integration consumers и
два host-теста профиля; внутри adapter production consumers нет. Профиль и
claim можно перенести целиком. Для сохранения host-тестов нужно отделить
esp-hal, аппаратный Wi-Fi adapter и optional SoC dependency в target-specific
таблицы, cfg-ограничить target composition/reexports и проверку network backend,
а host-тесты запускать явно в excluded integration. Pinned esp-hal build script
отклоняет host TARGET, поэтому одного `cfg` на Rust modules недостаточно.
Эта следующая manifest/cfg-схема ещё не проверена Cargo; её нельзя считать
завершённым переносом профиля.

## 0. Зафиксировать baseline и зависимости переноса

Результаты инвентаризации уже готовы: [files.csv](driver-audit/files.csv),
[tree.txt](driver-audit/tree.txt), [crates.md](driver-audit/crates.md),
[dependencies.csv](driver-audit/dependencies.csv). Перед первой кодовой волной
обновить snapshot относительно её фактической базы.

Дополнительно зафиксировать:

- public imports и compile-fail ownership contracts затрагиваемых crates;
- список реально обнаруженных тестов по соответствующим profiles;
- `cfg`, target feature profiles, linker attributes, task pool sizes;
- generated output paths, bindings crate identities и verification entrypoints;
- конкретные owners, права заёма, stop/quiesce/drop responsibilities;
- baseline target image placement и stack report для runtime-волн.

Корневой и isolated Wi-Fi graph сейчас используют разные esp-pacs revisions.
Для структурной baseline сохранять оба. Унификация revisions должна иметь
собственный diff и validation, а не происходить как случайное обновление lock.
Root `cargo metadata --no-deps` и resolved metadata трёх profiles уже успешно
проверены в аудите; это не заменяет baseline tests/builds перед изменением кода.

Критерий приёмки: все изменяемые файлы сопоставлены с owner/module и compiled
profiles; отсутствуют предположения, что корневой workspace охватывает всё.

## 1. Вынести тесты, не менять production module/API

Разделить на несколько независимых волн:

1. Portable: HCI command order, IEEE80211 station, AP service, WPA2 и STA helpers.
2. Memory/chip: Wi-Fi RX storage/ring, connected RX, HAL operation suites.
3. PHY/PAC: suites отдельных алгоритмов и handwritten restricted operations.
4. Adapters/integration: named suites, feature-gated owned tests, supervisor tests.

Для каждого inline suite сохранить его имя, исходный cfg, imports и родителей.
`foo.rs` получает `mod tests;`, тело уходит в `foo/tests.rs`; существующие
`owned_tests` и другие имена не превращать автоматически в `tests`.
Не менять production visibility, не переносить private tests в integration
crate. Compile-fail doctests остаются возле контракта.

`chips/esp32s31/ieee80211/mac/tests/mac.rs` разбить как один внешний test target с дочерними
модулями по поведению. Вынести common helpers в `tests/support/mod.rs`.

Критерии приёмки:

- production items/visibility/cfg неизменны;
- compiled test list совпадает при одинаковых features, либо различия в полном
  module path явно объяснены; ни один test target не исчез;
- те же tests проходят; не добавлены тесты адресов, masks или PAC type names;
- test-only helpers не попали в target release.

Обновление существующих проверок raw layout/констант до behavioral tests —
отдельная задача качества тестов. Перенос не должен незаметно удалить их или
ввести новые зеркальные проверки реализации.

## 2. Упорядочить модули внутри существующих crates

Сначала сохранять package names, public surface и affine boundaries.
Переносить целые операции/owners, а не нарезать файлы по числу строк.

| Область | Конкретная работа |
|---|---|
| PHY | `phy_*` -> `rx/tx/analog/calibration/tracking/hardware`, убрать повторы `phy_` |
| HAL | `owner`, `phy`, `wifi/arena`, `bluetooth`, `ieee802154`, `power` |
| Restricted PAC | `ownership`, `phy`, `wifi/mac`, `bluetooth`, `ieee802154` |
| Wi-Fi MAC | Сгруппировать `cold_*`, RX/TX, IRQ, HE, crypto |
| Bluetooth chip | Controller boot, interrupt publication, scheduler core и role attachment |
| Bluetooth Embassy | `controller/{owner,dispatch,reset,response}`, `session/{dtm,advertising,scan,peripheral}` |
| HCI | `transport`, `bootstrap`, `command/order` и command domains |
| Generic Embassy Wi-Fi | Capture из lib.rs -> `monitor/capture`; task helpers отдельными модулями |
| Radio facade | Явные `wifi` и `runtime::embassy`, без изменения Cargo direction |
| Wi-Fi integration | Child task/mailbox/rendezvous -> `execution/connected`; ISR bindings -> `interrupts` |

Для Bluetooth учитывать уже принятый [детальный план](BLUETOOTH_CODE_ARCHITECTURE_AUDIT.md):
pre-publication hardware crate возможен после изоляции scheduler boot,
interrupt publication и role attachment. Сразу создавать такой crate поверх
неразорванных зависимостей нельзя.

Критерии приёмки: поведение/public contracts сохранены, отсутствуют новые
`unsafe`, `Send`/`Sync`, `Clone` у affine owners; target attributes и stop order
сохранены. Для runtime переносов проверить post-LTO placement и stack frames.

## 3. Сделать provenance PAC и upstream bridge видимыми

### 3a. Разделить generated и handwritten trusted code

Составить точный список generated outputs и sidecars. Сейчас source of truth:
`registers/api.toml`, `vendor-project.toml`, `tools/blobray` publisher.
Generated Rust вручную не редактировать.

Первый вариант — минимальный: явные generated/bridge модули и ограниченный
lint scope. Сгенерированный код может иметь свои lint exceptions; handwritten
ownership sidecars должны проходить handwritten policy. Если перенос в
отдельный crate позволяет сохранить закрытые internals — использовать эту
более сильную границу. Не открывать raw access только ради crate split.

Переносить generated output в `src/generated/...` можно только вместе с
изменением publisher и проверкой повторной генерации. Простого `mv lib.rs`
недостаточно: generator дописывает API helpers/sidecar declarations.

### 3b. Разделить platform-pac по реальным обязанностям

Внутри бывшего `platform-pac` выделить typed upstream-register operations,
DMA descriptor codec, transfer owner и IRQ/future adaptation. Сохранить
текущий ABI/feature selection и singleton retention.

Отдельным пунктом записать unresolved contract CACHE maintenance/counters.
Новый CACHE lease, межъядерная сериализация или изменение writeback API
требуют самостоятельного regression-tested изменения и не являются `mv`.

Критерии приёмки: publish --check воспроизводим; bindings указывают на
скомпилированные production entrypoints; handwritten sidecars не маскируются
под generated lint exception; новые пути не открывают PAC наверх.

## 4. Нормализовать пути контейнеров

Полная пофайловая карта первого перемещения находится в
[files.csv](driver-audit/files.csv). Основные правила:

| До | После |
|---|---|
| `chips/esp32s31/pac-raw` | `chips/esp32s31/pac/raw` |
| `chips/esp32s31/platform-pac` | `adapters/esp-hal/esp32s31/soc` |
| `adapters/esp-hal/esp32s31-radio-platform` | `adapters/esp-hal/esp32s31/radio` |
| `adapters/esp-hal/esp32s31-{wifi,ieee802154}` | `adapters/esp-hal/esp32s31/{wifi,ieee802154}` |
| `adapters/embassy/esp32s31-platform` | `adapters/embassy/esp32s31/runtime` |
| Прочие `adapters/embassy/esp32s31-*` | `adapters/embassy/esp32s31/*` |
| `adapters/embassy-net` | `adapters/network/embassy/owned` |
| `adapters/embassy-net-compat` | `adapters/network/embassy/compat` |
| `adapters/research` | `adapters/network/research` |
| `integration/esp32s31/embassy-wifi` | `integration/esp32s31/embassy/wifi` |
| `integration/esp32s31/bluetooth` | `integration/esp32s31/embassy/bluetooth` |

Пути в таблице относительны `driver/`. Остальные контейнеры на этой волне
сохраняют расположение. Chip Wi-Fi compat bridge остаётся отдельным crate.

Для каждого moved package обновить одновременно:

- workspace members/exclude, relative path dependencies всех consumers;
- isolated workspaces, examples, HIL, verification probes;
- source path references, `include_*`, README links, qualification specifications;
- `tools/audit-driver-{architecture,safety}.sh` и source-only/metadata audits;
- publisher outputs, binding index и generator inputs при затрагивании PAC;
- linker/symbol-based ownership policies, если имена реально изменились.

Не переписывать историческое HIL evidence так, будто старый binary был собран
из новых путей. Актуальные исполняемые references обновляются; историческая
запись сохраняет commit и смысл, при необходимости добавляется mapping.

Критерии приёмки: все path dependencies разрешаются, исключённый workspace
по-прежнему проверяется отдельно, нет случайного dependency update и stale
publisher paths, весь source-only audit проходит.

## 5. Перенести обязанности через границы crates

Эта волна меняет архитектурные интерфейсы при сохранении поведения. Она
зависит от модульного разделения и должна идти отдельными небольшими PR.

### 5a. Portable codec / role policy / chip representation

1. Выделить descriptor alignment/queue encoding/callback representation из
   `ieee80211/mac` в chip memory/MAC модули.
2. Вынести local capability selection в chip profile; IE encoder оставить
   переносимым. Сохранить точные текущие capabilities и ограничения.
3. Переместить authentication/association retry/deadline state в `ieee80211/sta`.
4. Разделить portable secret и hardware key-upload representation, сохранив
   zeroization и существующие consumers до завершения перехода.
5. Выделить источник AP capacity, сохранив нынешнее значение и resource budgets.

Проверить consumers в compiled production и vendor comparison. Не заменять
production implementation копией в verification. Сравнение должно сохранять
`MATCH/DIFF/INCOMPLETE`, где отсутствие данных не превращается в успех.

### 5b. Chip runtime / Embassy / integration

1. Pure software queue contracts отделить от concrete network impls.
2. RX frontier/ring lifecycle/physical completion перенести в chip runtime
   modules; сначала использовать существующий Wi-Fi crate, новый runtime crate
   только при необходимости зависимостей/ownership barrier.
3. Timer/wake/select/channel оставить в Embassy; abstract delay contract
   отделить от concrete Embassy delay.
4. Production memory budgets/static placement собрать в integration.
5. По отдельности проверить orphan impl legality и DAG после каждого переноса.

Нельзя просто перенести `radio::embassy_supervisor` в существующий generic
Embassy Wi-Fi crate: сейчас radio зависит от него, а supervisor использует
типы radio — получится цикл. Ближайшее решение — явный module/feature boundary
в radio. Позднее возможен вынесенный control contract и направленный dependency
graph, если это даёт измеримую архитектурную пользу.

Критерии приёмки: portable protocol graph не содержит hardware dependencies;
chip owner не зависит от executor policy; command/channel handles не получают
PAC/DMA access; owned и compat сохраняют разные dependency closures; тесты и
current profiles дают прежние результаты.

## 6. Сократить имена и оценить объединение crates

Дополнительный анализ 2026-09-05:
[технические термины и protocol boundaries](DRIVER_PROTOCOL_NAMING.md).
Рекомендуемые family namespaces — `ieee80211`, `ieee802154`, `bluetooth`;
`wifi` остаётся допустимым прикладным facade, `wlan` не вводится как ещё один
синоним. Перенос контейнеров выполнен в пятой волне с прежними Cargo
package identities. Конкретная карта и находки ESP-NOW/WMM/SoftMAC,
HCI command authority и IEEE 802.15.4 ISR ownership находятся в документе.
Следующий этап — внутренние protocol/extension/owner boundaries;
после них сокращать типы в устойчивых namespaces.

Сначала public module namespaces и dependency aliases `oer`/контекстные
`radio_hal`, `wifi_mac`, `net`; затем короткие типы внутри них. Удалять redundant
chip/protocol/executor prefixes, сохраняя ownership/state vocabulary.

Полный package rename `open-esp-radio-*` -> `oer-*` — отдельная волна после
стабилизации путей. Перед ней нужен полный symbol/manifest/binding consumer map.
Сохранить существующий crate name возможно; короткий local dependency alias
уже решает большую часть повседневной многословности.

Кандидаты на consolidation: portable STA/AP/SoftMAC и chip role facades.
Для каждого объединения отдельно доказать:

- отсутствие dependency cycles;
- сохранение запрета неправильных dependencies/unsafe;
- отсутствие ненужного crypto/fork/target feature fanout;
- сохранение memory ownership/privacy и публичных контрактов;
- понятный выигрыш в навигации и surface area.

Если это не доказано, оставить crate и организовать его модули. Crate count
не является целевой метрикой.

## 7. Синхронизировать документацию и закрепить границы

Обновлять локальные README/UNSAFE одновременно с соответствующим переносом,
а на завершающей волне привести всю навигацию к целевой карте. Не поддерживать
две конкурирующие нормативные архитектуры.

Усилить проверки по реальному контракту:

- Cargo metadata dependency closures для запрещённых направлений;
- scoped unsafe lint policy для generated и handwritten областей;
- production ownership behavioral tests и compile-fail public contracts;
- publisher reproducibility и binding validation;
- target placement/stack reports для runtime boundaries.

Не заменять эти проверки regex-тестами имён типов/файлов или тестами PAC masks.
По завершении переноса удалить временные aliases и актуализировать либо убрать
этот migration plan из списка текущих документов.

## Проверки для кодовых волн

Для focused iteration выбирать соответствующие packages и features, затем
выполнить обязательный набор проверки перед завершением переноса:

```console
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo qualification validate --manifest qualification/targets/esp32s31/wifi-sta.toml
tools/audit-cargo-metadata.sh
tools/audit-driver-architecture.sh
tools/audit-driver-safety.sh
tools/audit-source-only.sh
```

Architecture audit читает `supported-feature-profiles` и компилирует их
отдельно. Для ручной локальной проверки excluded integration:

```console
cargo check --locked --offline --target riscv32imafc-unknown-none-elf \
  --manifest-path driver/integration/esp32s31/embassy/ieee80211/Cargo.toml \
  --no-default-features --features owned-network
cargo check --locked --offline --target riscv32imafc-unknown-none-elf \
  --manifest-path driver/integration/esp32s31/embassy/ieee80211/Cargo.toml \
  --no-default-features --features compat-network
cargo fmt --manifest-path driver/integration/esp32s31/embassy/ieee80211/Cargo.toml -- --check
```

Это минимальные профили для итерации, а не замена diagnostic profiles из
package metadata.

При изменении PAC layout/publisher:

```console
cargo blobray project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml --check
```

Если потребуется настоящий vendor analysis, запускать через
`tools/blobray/scripts/run-limited` по repository guidelines. Старое сравнение
с другой production entry не является доказательством нового binding.

Для runtime/placement волн использовать текущий build/report workflow из
[HIL target README](../hil/targets/esp32s31/README.md) и
[reproducibility contract](HIL_BUILD_AND_REPORT_REPRODUCIBILITY.md).
HIL требует подключённой платы; при отсутствии hardware фиксировать отсутствие
HIL validation и не заявлять новое qualification. Изменение source paths не
гарантирует тождественный ELF после LTO, особенно для больших async futures.

## Явно отдельные будущие задачи

Общий Wi-Fi/Bluetooth platform coordinator и включение совместных режимов;
новые CACHE ownership/API guarantees; унификация esp-pacs revisions;
расширение capability profiles; изменение queue fairness/packet scheduling;
реализация отсутствующих Bluetooth/802.15.4 runtime возможностей.

Эти задачи выявлены аудитом, но не должны скрываться внутри структурного
переноса. Для них нужны отдельные behavior contracts, regression tests и
соответствующая evidence. Смена пути или короткое имя типа не решает их.
