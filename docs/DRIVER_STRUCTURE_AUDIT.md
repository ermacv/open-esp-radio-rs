# Аудит структуры driver

Дата: 2026-09-04. База: `134a75ac6f0eeeb60a76fca22d0bfbf51b1f4013`.
Статус: исходный снимок аудита. Реализация структурной миграции начата;
выполненные изменения и оставшиеся границы отмечены в [плане](DRIVER_STRUCTURE_PLAN.md).
Числа и текстовые ссылки на исходные строки ниже относятся к базе аудита.

Главная рекомендация — сохранить полезные границы PAC, HAL, DMA и протоколов,
но сделать их видимыми в путях и Rust-модулях. Основная проблема сейчас —
смешанные обязанности внутри некоторых крейтов и плоские пространства имён.
Количество крейтов само по себе не объясняет сложность. Сначала нужны вынос
тестов и разбиение модулей, затем перенос ответственности между крейтами;
переименование пакетов и их объединение — отдельные изменения.

Аудит выполнен основным агентом и тремя независимыми агентами по направлениям
hardware/PAC, Embassy/integration и portable protocols/tests. Все отслеживаемые
файлы и манифесты включены в инвентаризацию. Семантически исследованы границы,
публичные поверхности, владельцы, ключевые реализации и потребители; это не
построчная верификация полумиллиона строк и не доказательство отсутствия ошибок
владения во всех ветвях исполнения.

## 1. Полная карта текущего дерева

| Измерение | Результат |
|---|---:|
| Отслеживаемые файлы в driver | 767 |
| Каталоги с отслеживаемыми потомками, включая driver | 148 |
| Cargo packages | 44: 43 в корневом workspace, 1 отдельный workspace |
| Rust-файлы | 713 |
| Строки Rust, включая комментарии, тесты и пустые строки | 503 079 |
| Два основных generated Rust output | 78 678 строк |
| Markdown-файлы | 9 |
| Атрибуты `#[test]`, текстовый подсчёт | 2248 |
| Файлы с `#[test]` | 363 |
| Inline-модули с буквальным именем `tests` | 336 |

Полные приложения:

- [Дерево всех каталогов и файлов](driver-audit/tree.txt), с размером и видом каждого файла.
- [Реестр всех 44 крейтов](driver-audit/crates.md), с текущими и предлагаемыми корнями.
- [Пофайловый реестр](driver-audit/files.csv): владеющий crate, слой, generated/handwritten/test,
  строки, тестовые маркеры, предлагаемый путь контейнера.
- [256 объявленных зависимостей](driver-audit/dependencies.csv), включая dev, optional и target cfg.
- [План миграции и критерии приёмки](DRIVER_STRUCTURE_PLAN.md).

CSV не означает, что каждый файл уже разобран на целевые функции. Поле
`proposed_container_path` — однозначная карта первого перемещения контейнеров.
Последующие внутренние разделения указаны ниже и в плане. Строки generated
output посчитаны отдельно: большой generated `lib.rs` не является таким же
архитектурным дефектом, как большой рукописный controller actor.

Источником инвентаризации служит `git ls-files driver`, а не рекурсивный обход
всего диска. Игнорируемые `target/`, сборочные документы и артефакты не входят в
код драйвера. В `driver` всего девять Markdown-файлов; существенная доля
пояснений находится в Rust doc comments.

### Ответственности всех верхних ветвей

| Ветка | Фактическая роль | Решение |
|---|---|---|
| `radio` | Публичные requests/lifecycle плюс generic Embassy supervisor | Явно разделить `wifi` и `runtime::embassy` внутри crate |
| `common/dma` | Pinned memory, affine handoffs и SPSC | Сохранить audited memory crate |
| `common/network` | Нейтральные interface/link/error значения | Сохранить малый leaf; не присоединять к unsafe DMA |
| `wifi/ieee80211` | Wire codec, часть STA policy, часть S31 representation | Очистить границы codec / policy / chip profile |
| `wifi/{sta,ap,softmac}` | Переносимые роли, конфигурация, monitor/ESP-NOW | Группировать модулями; не объединять немедленно |
| `wifi/wpa2` | Crypto, EAPOL, key ownership | Сохранить самостоятельную границу секретов |
| `wifi/datapath` | Нейтральные egress/materialization ownership contracts | Сохранить общий contract leaf |
| `bluetooth/hci` | HCI transport, bootstrap, command ordering | Разделить пространства `transport/bootstrap/command` |
| `bluetooth/ll` | Переносимые PDU и LL role state | Сохранить независимость от chip/HCI runtime |
| `ieee802154` | Frames, metadata, commands/events | Один crate с модулями уже подходит |
| `chips/esp32s31` | Reviewed radio PAC, HAL/PHY, MAC/DMA и chip owners | Сохранить chip boundary; подробности ниже |
| `adapters/embassy` | Async primitives, executor runtime, крупные chip runtimes | Разделить механизм ожидания и backend responsibilities |
| `adapters/esp-hal` | Singleton tokens, CPU IRQ routes, platform hooks | Собрать по `esp-hal/esp32s31/...` |
| `adapters/embassy-net*` | Два несовместимых сетевых API/ownership режима | Сохранить разные crates, сгруппировать путями |
| `adapters/research` | Synchronous network engine и physical materialization | Оставить production leaf, назвать `network/research` |
| `integration/esp32s31` | Static placement, facade, composition, task/ISR binding | Оставить composition root, явно выделить execution |

## 2. Находки, которые определяют архитектуру

Приоритет здесь означает порядок структурной работы, а не severity ошибки
драйвера. Ниже разделены наблюдаемые факты и предлагаемые изменения.

### A. Generated и handwritten доверенные слои смешаны

`pac-raw/src/lib.rs` действительно генерируется, но рядом находятся
`ieee802154_mac_ownership.rs` (1731 строка) и два validation sidecar.
`registers/api.toml:187` подключает ownership sidecar. Его задача — потребить
единый MAC owner, выдать отдельные task/ISR handles и затем соединить их.
При этом `tools/audit-driver-safety.sh:12–16` классифицирует весь raw package как
сгенерированный, а `pac-raw/Cargo.toml:23` разрешает
`unsafe_op_in_unsafe_fn` для всего пакета.

Это установленное расхождение provenance и охвата lint-аудита; оно само по
себе не доказывает unsafe bug. Нужно явно обозначить generated output и
рукописный trusted bridge. Минимальный шаг — модули/каталоги и отдельная lint
область; более сильный — отдельный generated crate и handwritten restricted
crate, если bridge не требует открытия generated-private internals.

Исходники: [raw manifest](../driver/chips/esp32s31/pac/raw/Cargo.toml),
[ownership sidecar](../driver/chips/esp32s31/pac/raw/src/ieee802154_mac_ownership.rs),
[safety audit](../tools/audit-driver-safety.sh).

### B. platform-pac фактически является SoC HAL и DMA runtime

В одном crate находятся `FlashMmu`, `L1CachePerformanceCounters`, cache
writeback, descriptor codec, DMA transfer owners, `Future`, `AtomicWaker` и
IRQ handler. В `axi_gdma_mem2mem.rs:198,228` видны отдельные владельцы памяти и
DMA channel. Это больше, чем register access.

Источник регистров — закреплённый fork `esp-hal`, через типизированные
accessors его периферии; прямой зависимости на crate `esp32s31` здесь нет.
Целевое место: `adapters/esp-hal/esp32s31/soc`. Внутри явно отделить
`registers::{cache,flash,gdma}` от `dma::{descriptor,transfer,irq}`.
Вначале это один существующий crate с модулями.

Важная отдельная ownership-зона для последующего рассмотрения:
`cache_maintenance.rs:53` принимает memory borrow и выполняет сериализацию
через critical section, но не принимает CACHE singleton token. Поэтому
контракт «все операции CACHE доказываются владением одним CACHE token» сейчас
неверен. Нужно описать совместное использование maintenance и counters;
добавление нового owner/borrow API уже требует отдельного изменения поведения
и анализа межъядерной сериализации.

### C. Wi-Fi и Bluetooth имеют разные пути acquisition общих ресурсов

`adapters/esp-hal/esp32s31-wifi/src/lib.rs:37–46` удерживает WIFI и общие
platform singleton tokens. `esp32s31-radio-platform/src/esp32s31.rs:29`
удерживает пересекающийся набор для coordinator/Bluetooth route.
`README.md:17` этого adapter прямо запрещает safe совместную Wi-Fi+Bluetooth
composition до миграции. Дополнительно `pac::RadioHardware`
(`pac/src/lib.rs:385–458`) предоставляет exclusive protocol route.

Это ограничение архитектуры, а не найденное двойное владение: singleton API
как раз препятствует повторному safe acquisition. Целевой общий coordinator
полезен, но объединение его lifecycle, protocol leases и scheduler выходит за
механическое структурирование. Нельзя включать coexistence попутно с `mv`.

### D. Embassy adapter скрывает большой hardware runtime

`adapters/embassy/esp32s31-wifi` содержит 157 Rust-файлов и 68 946 строк.
Внутри есть pure owner queues (`datapath/software_tx_queue.rs`), chip RX ring
state (`datapath/rx/frontier/state.rs`), delay contract вместе с Embassy timer
(`frontier/time.rs`), сетевой contract рядом с owned implementation
(`datapath/network.rs`) и integration budgets (`composition/resources.rs`).

Целевые владельцы этих обязанностей:

| Обязанность | Целевой слой |
|---|---|
| Независимые queue/flow ownership transitions | `wifi/datapath`, после проверки chip assumptions |
| RX ring frontier, DMA epochs, physical completion | `chips/esp32s31/wifi` runtime-модули |
| `Signal`, `Channel`, `Timer`, `select`, wake plumbing | `adapters/embassy` |
| Конкретный network trait impl | `adapters/network` или chip network bridge |
| Static capacities, placement, task assembly | `integration` |

Это пофункциональная классификация. Переносить всё поддерево `datapath` в
portable crate нельзя: там есть аппаратное владение и concrete dependencies.
Часть общих helper traits можно выделить в модули существующего crate до
изменения Cargo DAG.

### E. Один физический владелец не означает одну Embassy task

Контракт в `radio/src/embassy_supervisor.rs:69–75` и комментарий
`integration/.../supervisor/mod.rs:499–505` можно прочитать как требование
держать hardware owner непосредственно в supervisor future. Реальный STA
путь передаёт `ConnectedDatapathRunner` в отдельную task и возвращает **тот же**
owner через локальный `!Sync` RefCell rendezvous:
`supervisor/station.rs:390–525,2040–2062`.

Видны completion wait и возврат owner перед quiesce; это не доказанная потеря
владения. Корректнее документировать один ownership domain Core0 с supervisor
и управляемым дочерним actor. Signal сообщает о событии, mailbox переносит
owner, supervisor ждёт возврата. Task/rendezvous следует вынести в
`integration/.../execution/connected.rs`; сохранить pool size, stop order,
локальность и поведение cancellation/drop. Одна папка `embassy` не доказывает
эти свойства.

### F. Portable IEEE802.11 содержит chip representation и локальный профиль

| Место | Смешанная ответственность | Куда выделить |
|---|---|---|
| `ieee80211/src/alignment.rs:3–41` | Descriptor `storage_word` transformation | Chip Wi-Fi memory/descriptor module |
| `ieee80211/src/data.rs:335–357` | Vendor queue numbers, packed descriptor byte, callback mask | Chip MAC representation; portable слой возвращает typed category/intent |
| `ieee80211/src/station.rs:633–640` | S31 `cbw: u8`, channel/frequency union | Typed portable selection + chip lowering |
| `station.rs:38–162,1147–1165` | S31/vendor/HIL-selected local capabilities | Chip capability profile + portable IE encoder |
| `ieee80211/src/ht.rs:86–94` | Advertised capacity из S31 RX descriptor budget | Chip profile |
| `station.rs:697,759,904,976` | Retry/deadline/authentication/association state | `wifi/sta` |
| `wpa2/src/keys.rs:19–23` | Secret owner alignment ради S31 | Chip key-upload binding; secret/zeroization остаются в WPA2 |
| `wifi/ap/src/service.rs:29–34` | Предел 15 peers из S31 key slots | Backend/profile capacity |

Это не MMIO leak и не доказательство неправильного пакета в эфире. Выделение
должно сохранить нынешние байты IE, queue mapping, key lifetime/zeroization,
лимиты и fail-closed результаты. Изменять протокольные параметры, key ABI или
вводить произвольные capacity одновременно с переносом не нужно.

### G. Есть реальная независимая матрица сборки и drift PAC revision

`driver/integration/esp32s31/embassy-wifi` исключён из корневого workspace и
имеет собственные `[workspace]`, `[patch]`, lockfile и альтернативные profiles.
Проверено `cargo metadata --locked --offline --filter-platform
riscv32imafc-unknown-none-elf`:

| Граф | esp-hal rev | esp-pacs ESP32-S31 rev | Network |
|---|---|---|---|
| Root workspace | `81cd5c341f71` | `85d2b4ddde20` | Совокупность workspace packages |
| Isolated Wi-Fi, owned | `81cd5c341f71` | `5b8b56036abd` | Fork Embassy/Xarxa |
| Isolated Wi-Fi, compat | `81cd5c341f71` | `5b8b56036abd` | Released Embassy |

Различие ревизий подтверждено и lockfiles, и resolved metadata. Это не
утверждение о двух PAC-владельцах в одной прошивке и не доказательство
несовместимости revisions. Но формулировка «проект использует один закреплённый
PAC» сейчас неточна. До унификации версий надо сопоставить consumers и
qualification inputs; обновление зависимости — отдельная проверяемая задача.
`cargo check --workspace` в корне не проверяет excluded integration.
`--all-features` для Wi-Fi composition некорректен: owned/compat альтернативны.

### H. Документация отстаёт от реализованных границ

`driver/README.md:67` ссылается на отсутствующую ветку `registers`.
`UNSAFE.md:8` называет закрытый `pac` generated MMIO и перечисляет меньше
исключений, чем исполняемый safety audit. Driver README одновременно хранит
архитектуру, capability frontier и сведения о прошлых этапах работы.

Нужен короткий навигационный README, актуальный UNSAFE contract и ссылки на
единственные источники состояния функций. Архитектура не должна обещать
Bluetooth/802.15.4 readiness по самому факту существования папки. Датированное
HIL evidence остаётся в qualification, vendor comparison — в verification.

## 3. Целевые аппаратные границы и esp-hal / esp-pacs

Здесь HAL означает radio hardware abstraction данного проекта. Это не
реализация всего MCU HAL и не попытка заменить `esp-hal`.

| Слой | Чем владеет | Что не должно сюда попадать |
|---|---|---|
| Raw generated PAC | Register-local fields и typed accessor machinery | Role policy, sleep/retry, executor |
| Restricted PAC | Reviewed register operations, partitions, split/reunite witnesses | Общий scheduler, network queues, recovery policy |
| Radio HAL | Hardware lifecycle, сериализация arena, polling/delays, multi-operation sequencing | Association/security policy, network stack |
| PHY | Calibration/channel/RF algorithms и их state | Raw PAC access, платформенный singleton acquisition |
| Chip MAC/DMA/backend | Descriptors, ring ownership, LMAC, аппаратный lowering | DHCP/sockets, portable role policy |
| esp-hal binding | Принятые MCU singleton witnesses, clocks/reset hooks, CPU IRQ routes | Второй singleton root для тех же регистров |
| Embassy binding | Waiting/waking, time/executor adaptation | Смысл MAC operation или протокольного события |
| Integration | Размещение ресурсов и сборка конкретного сервиса | Новая shadow implementation аппаратных операций |

Существующие `PhyHal`, `Radio<P, State>`, `RadioRuntimeOwner` и guarded arena
полезны. `Copy` у borrowed capability не означает свободное владение MMIO;
сериализация остаётся у arena, а lifetime должен оставаться связан с ней.
`RefCell` контролирует локальные заимствования в одном execution domain, но
сам по себе не доказывает безопасность между cores или ISR.

### Две цепочки register provenance

```text
Reviewed target register model + reviewed API pack
  -> Blobray register-model / publisher / svd2rust
  -> svd/esp32s31-radio.svd + bindings
  -> custom raw PAC + generated semantic domains
  -> handwritten restricted PAC -> radio HAL -> PHY/MAC

Pinned esp-pacs source
  -> pinned esp-hal peripheral accessors + singleton witnesses
  -> adapters/esp-hal/esp32s31/{soc,radio,wifi,ieee802154}
  -> platform operations / CPU IRQ routes used by composition
```

`pac-gen` — build-time tooling. Сейчас его обязанности уже находятся в
`tools/blobray/crates/register-model` и `tools/blobray/src/registers/pac.rs`.
Новый `driver/pac-gen` не нужен. Источник редактирования — reviewed model в
`verification/vendor/targets/esp32s31/registers`, а SVD является output;
ручное исправление generated Rust/SVD неправильно для текущего pipeline.
См. [svd/README.md](../svd/README.md).

`vendor-project.toml:366–380` задаёт четыре output: SVD, raw PAC, bindings index
с именем Rust crate, generated semantic module. Перенос PAC должен обновить
этот publisher contract. Платформенный
`svd/esp32s31-platform-radio-deps.svd` используется валидатором; он не создаёт
runtime crate или второй peripheral owner. Сохранить описанный carveout
общего PHY `MODEM_LPCON::TICK_CONF`, не объединять apertures по похожим именам.

В локально закреплённом `esp-hal` (`81cd5c341f71`) `peripherals/mod.rs:15`
импортирует PAC приватно, а `:149–169` выдаёт `PTR`, `regs()` и
`register_block()` через peripheral wrappers. Поэтому архитектурный план не
должен опираться на выдуманный публичный `esp_hal::pac`. Наш handwritten bridge
должен пользоваться именно доступными typed accessors и принятыми ownership
witnesses; без нового `Peripherals::take/steal` поверх уже принятого HAL owner.
Источник: [закреплённый esp-hal peripheral wrapper](https://github.com/ermacv/esp-hal/blob/81cd5c341f71f9d070b5d2b115d3ab2c7595a4df/esp-hal/src/peripherals/mod.rs).

Если в официальном PAC не хватает поля, изменение готовится в SVD/patch
pipeline `esp-pacs`, затем закрепляется совместимая ревизия. Репозиторий
`esp-pacs` отдельно хранит SVD patches и генерацию через `xtask`; это полезный
пример разделения inputs, generator и outputs. Это не основание переносить
все radio-specific reviewed sequences в upstream PAC.
[Источник esp-pacs](https://github.com/esp-rs/esp-pacs#patching-the-svds).

## 4. Предлагаемая иерархия

`[crate]` обозначает существующую границу пакета, сохраняемую в первой фазе.
Остальные элементы — каталоги/модули, не новые Cargo packages. Наличие
вложенного crate не создаёт автоматически Rust namespace.

```text
driver/
  README.md
  UNSAFE.md
  radio/ [crate]
    src/{wifi/,runtime/embassy.rs,...}
  common/{dma/,network/} [crates]
  wifi/
    ieee80211/ [crate]       src/{frame/,ie/,management/,data/,...}
    sta/ [crate]             src/{scan/,authentication/,association/,power/,...}
    ap/ [crate]              src/{bss/,peer/,power/,security/,...}
    softmac/ [crate]         src/{interface/,monitor/,esp_now/,...}
    wpa2/ [crate]            src/{key/,eapol/,supplicant/,authenticator/,...}
    datapath/ [crate]        src/{egress/,flow/,materialization/,...}
  bluetooth/
    hci/ [crate]             src/{transport/,bootstrap/,command/,...}
    ll/ [crate]              src/{advertising/,scanning/,connection/,...}
  ieee802154/ [crate]
  chips/esp32s31/
    pac/ [crate]             src/{ownership/,phy/,wifi/,bluetooth/,ieee802154/,...}
      raw/ [crate]           generated backend + explicitly tracked sidecar boundary
    hal/ [crate]             src/{owner/,phy/,wifi/,bluetooth/,ieee802154/,power/}
    phy/ [crate]             src/{state/,calibration/,rx/,tx/,analog/,tracking/,hardware/}
    wifi/ [crate]            src/{runtime/,datapath/,profile/,...}
      dma/ [crate]
      mac/ [crate]           src/{cold/,rx/,tx/,he/,crypto/,irq/,...}
      sta/ [crate]
      ap/ [crate]
    bluetooth/ [crate]       src/{controller/,scheduler/,advertising/,scanning/,
                                  connection/,dtm/,interrupt/,phy/}
      memory/ [crate]
    ieee802154/{dma/,irq/,mac/,runtime/} [crates]
    coex/ [crate]
  adapters/
    network/
      embassy/{owned/,compat/} [crates]
      research/ [crate]
    esp-hal/esp32s31/
      soc/ [crate]           src/{registers/,dma/,...}; former platform-pac
      radio/ [crate]         singleton coordinator, protocol hooks
      wifi/ [crate]
      ieee802154/ [crate]
    embassy/
      wifi/ [crate]          generic async contracts / capture / task helpers
      esp32s31/
        runtime/ [crate]     executor and time-driver singleton
        wifi/ [crate]        wake/time/async orchestration
        wifi-compat/ [crate] chip bridge for released network contract
        bluetooth/ [crate]   controller/session orchestration
        ieee802154/ [crate]
        coex/ [crate]
  integration/esp32s31/
    embassy/wifi/ [isolated crate]
      src/{facade/,resources/,interrupts/,execution/,supervisor/,network/,status/}
    embassy/bluetooth/ [crate]  placement + concrete Embassy/esp-hal composition
```

Bluetooth integration уже содержит конкретную Embassy composition:
`system.rs:4–25` компонует select/yield, controller command task и modem timer;
`interrupt_runtime.rs:8–42` использует конкретные Embassy wakers. Поэтому
предлагается `integration/esp32s31/embassy/bluetooth`, сохраняя существующий
crate и target cfg. Внутри отдельно обозначить нейтральные resource placement
модули и concrete execution. Для portable HCI вывод иной: там reusable
`embassy-sync` primitives сами по себе не означают executor ownership.

Для raw PAC первый этап сохраняет generated `lib.rs` и sidecars под отдельным
описанным audit contract. Дальнейшее расположение `src/generated/` и
`src/bridge/` требует изменения publisher/module layout; такую структуру нельзя
заявить достигнутой простым переименованием package directory.

### Правило crate или module

Отдельный crate нужен, если он удерживает хотя бы одну проверяемую границу:
запрет PAC/unsafe, собственный target ABI/linker/time-driver singleton,
независимый dependency/feature graph, несколько внешних потребителей общего
контракта или отдельно проверяемый memory ownership protocol.

Внутренние phases, PHY algorithms, MAC RX/TX, controller roles, dispatch,
response и тестовые suites становятся модулями. Не создавать crate на каждый
state machine или на каждый уровень пути.

| Решение | Области | Причина |
|---|---|---|
| Сохранить | Raw/restricted PAC, HAL, PHY | Register/unsafe/dependency authority |
| Сохранить | Common DMA, Wi-Fi DMA, Bluetooth memory | Отдельные memory proofs и закрытые codecs |
| Сохранить | Network owned/compat, chip compat bridge | Изоляция fork/released graph |
| Сохранить | Embassy platform runtime | ABI, IRQ symbols, timer singleton, target cfg |
| Сохранить | IEEE codec, WPA2, datapath, HCI, LL, common/network | Разные contracts/dependencies и потребители |
| Рассмотреть позже | Wi-Fi STA/AP/SoftMAC | AP тянет WPA2, STA нет; SoftMAC — нижний contract |
| Рассмотреть позже | Chip STA/AP вместе с backend | Сначала разорвать зависимости и проверить public consumers |
| Не сливать механически | 802.15.4 IRQ/runtime и coex | HAL уже зависит от IRQ/coex; укрупнение может создать цикл |

Уменьшение числа crates не является самостоятельным критерием успеха.
Например, `common/network` содержит лишь 58 строк, но не заставляет сетевые
adapters зависеть от hardware, allocator или unsafe DMA.

## 5. Владение: что должно оставаться явным

| Ресурс | Приобретение/хранение | Заём/публикация | Завершение |
|---|---|---|---|
| Custom radio PAC | Neutral `RadioHardware`, exclusive route | Restricted partitions, HAL capabilities | Только разрешённые typestate transitions |
| MCU platform tokens | esp-hal binding/coordinator | Protocol-specific hooks/leases | Не дублировать acquisition между протоколами |
| PHY/runtime arena | HAL lifecycle owner | Borrowed serialized capability | Не раскрывать PAC callback/Deref |
| CPU interrupt route | esp-hal route binding | Stable target entrypoint | Disable on required core before recovery |
| Peripheral IRQ registers | Chip/PAC epoch owner | Узкая ISR capability | Acknowledge/drain/mask по protocol contract |
| ISR notification | Embassy queue/signal | Value snapshot/wake | Wi-Fi coalescing не заменяет 802.15.4 event queue |
| TX software packet | Network adapter / flow queue | Selected complete batch | Terminal radio completion освобождает physical pool |
| DMA-visible SRAM | Chip DMA owner + integration placement | Affine publication in hardware | Complete/reclaim или quarantine |
| STA child runner | Supervisor rendezvous, затем child task | Тот же owner в пределах Core0 | Completion + exact owner return + quiesce |
| HCI endpoints | Portable bounded transport | Epoch-bound affine split | Не выдавать chip/session owner через transport |
| Stack/DHCP/sockets | Application | Driver tokens/link state | Application-owned network task |

Не унифицировать protocol-specific IRQ primitives только из-за одинаковых
слов `Signal`, `Channel` или `Event`: Bluetooth classified wakes и 802.15.4
очередь acknowledged snapshots имеют другой смысл, чем Wi-Fi pending wake.

`embassy-net-driver::Driver` представляет token-based обмен кадрами и link
state; сетевой `embassy_net::Runner` отдельно исполняет stack. Это поддерживает
разделение network adapter, radio runtime и application-owned stack.
[Driver API](https://docs.rs/embassy-net-driver/0.2.0/embassy_net_driver/trait.Driver.html),
[network Runner](https://docs.rs/embassy-net/0.9.1/embassy_net/struct.Runner.html).

`embassy-sync` в portable HCI использован как mutex/waker vocabulary без spawn,
timer или MMIO. Его наличие само по себе не является нарушением executor
ownership и не требует перемещения HCI в adapter.

## 6. Названия и сокращение oer

Рекомендация: `oer` использовать сначала как имя dependency/import alias,
сохраняя package identity. Например, в потребляющем crate:

```toml
[dependencies]
oer_hal = { package = "open-esp-radio-esp32s31-hal", path = "<relative-path-to-hal>" }
```

Путь в примере схематический; конкретное значение зависит от consumer.
Cargo поддерживает такое переименование зависимости через `package`.
[Cargo reference](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#renaming-dependencies-in-cargotoml).

Для публичного facade возможно короткое `oer`, для внутренних импортов —
контекстные `radio_hal`, `wifi_mac`, `wifi_dma`, `net`, если это яснее, чем
повторяющийся `oer_esp32s31_...`. Полный rename пакетов в `oer-*` разумен только
отдельной согласованной волной: он затрагивает Cargo, bindings, safety
allowlists, doctests, examples, HIL и qualification references. Доступность
имён в registry в этом аудите не проверялась и не требуется для локальных aliases.

| Сейчас | Предлагаемый namespace |
|---|---|
| `phy/src/phy_rxiq.rs` | `phy::rx::iq` |
| `phy/src/phy_rx_gain_cal.rs` | `phy::calibration::rx_gain` |
| `mac/src/cold_antenna.rs` и другие `cold_*` | `mac::cold::{antenna,...}` |
| `Esp32s31RadioOwnerArena` | `hal::wifi::arena::Arena` |
| `Esp32s31RadioOwnerRepublish` | `hal::wifi::arena::Republish` |
| `EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationReady` | `bluetooth::advertising::connectable::recurring::CancellationReady` |
| `hci/src/dtm_order.rs`, уже обслуживающий другие команды | `hci::command::order` |

Не сокращать смысловые окончания `Owner`, `Lease`, `Handle`, `Prepared`,
`Published`, `Reclaimed`, `Faulted`: они показывают разные права и состояния.
Не экспортировать все короткие имена плоским `pub use` в root: сначала
определить публичные модули, затем сокращать типы. В переходной фазе допустимы
ограниченные reexports с заранее определённым этапом удаления.

## 7. Тесты и документация

### Вынос тестов

Для private unit tests сохранить вложенность Rust module:

```text
src/station.rs          -> #[cfg(test)] mod tests;
src/station/tests.rs    -> use super::*; существующие тесты
```

Для `lib.rs` дочерний файл — `src/tests.rs`. Для нескольких suites сохранить
их существующие имена/cfg и использовать отдельные файлы. В частности,
`#[cfg(all(test, feature = "owned-network"))]` нельзя заменить общим cfg.
Не увеличивать `pub` ради доступа к тестам. Публичные integration contracts
остаются в `<crate>/tests`, где тесты компилируются как внешние потребители.
Это соответствует Rust module model и организации тестов.
[Rust modules](https://doc.rust-lang.org/reference/items/modules.html),
[Rust test organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html).

| Первые кандидаты | Строк всего | Начало test block, ориентир |
|---|---:|---:|
| `bluetooth/hci/src/dtm_order.rs` | 3331 | 1501 |
| `wifi/ieee80211/src/station.rs` | 3489 | 2268 |
| `wifi/ap/src/service.rs` | 3369 | 2253 |
| `chips/esp32s31/wifi/sta/src/connected_rx.rs` | 4271 | 2564 |
| `chips/esp32s31/wifi/dma/src/rx_storage.rs` | 2332 | 1244 |
| `chips/esp32s31/hal/src/ieee802154_operation.rs` | 2061 | 1213 |

Текстовые 2248 `#[test]` — ориентир, не число успешно исполняемых тестов.
Есть и иные имена suites, doctests, compile-fail tests и cfg-зависимые ветви.
До/после каждой волны сравнивать compiled test lists для тех же profiles.
Сохранить доказательства affine ownership в compile-fail doctests.

`chips/esp32s31/wifi/mac/tests/mac.rs` уже вынесен, но содержит 3951 строку.
Его можно разделить на дочерние suites по публичным контрактам внутри одного
test target, с `tests/support/mod.rs`, не порождая отдельный binary на каждый
маленький файл.

Вынос тестов не решит production-монолиты:
`controller_command_task.rs` имеет 5262 строки до tests,
`controller_start.rs` — 4864. Они требуют отдельных controller/interrupt/
role/phase модулей. Подробный Bluetooth план следует согласовать с уже
существующим [Bluetooth architecture audit](BLUETOOTH_CODE_ARCHITECTURE_AUDIT.md),
особенно с отложенным выделением pre-publication hardware boundary.

### Место всех девяти Markdown-файлов

| Текущий документ | Целевое назначение |
|---|---|
| `driver/README.md` | Краткая навигация, слой/owner map, ссылки на contracts |
| `driver/UNSAFE.md` | Точная unsafe/generated/bridge карта, согласованная с tooling |
| `adapters/research/README.md` | Контракт production network leaf; перенос вместе с crate |
| `adapters/esp-hal/esp32s31-radio-platform/README.md` | Token/coordinator/exclusive route contract |
| `chips/esp32s31/pac-raw/README.md` | Generated и sidecar provenance, команды publish/check |
| `chips/esp32s31/wifi/FEATURES.md` | Текущая Wi-Fi frontier, ссылки на qualification |
| `chips/esp32s31/bluetooth/FEATURES.md` | Текущая Bluetooth frontier без архитектурных обещаний |
| `chips/esp32s31/wifi/sta/README.md` | Chip STA port contract |
| `wifi/sta/README.md` | Portable STA policy contract |

Длинные общие архитектурные объяснения держать в docs. После миграции этот
датированный план нужно заменить актуальным контрактом или удалить из текущей
навигации; git хранит историю. Не создавать параллельные копии capability status.
SOURCE/provenance-комментарии возле операций сохранять: это часть связи
production implementation с reviewed evidence.

## 8. Что проверено сейчас и что остаётся проверять при переносах

В ходе аудита успешно прочитаны root metadata без dependencies и три resolved
metadata graph: root, isolated owned и isolated compat, все locked/offline,
для `riscv32imafc-unknown-none-elf` в resolved случаях. Объявленные local path
edges проверены отдельно; учтены nested crates и excluded workspace.

На момент составления исходного аудита production Rust, manifests, generated
outputs и lockfiles не изменялись. Последующая реализация описана в плане. Полные cargo test/check/clippy и HIL для документационного аудита
не запускались. Соответствие текущей аппаратной логики и qualification этим
отчётом не заявляется.

При реализации нужны host tests, feature-specific target builds, architecture/
safety/source audits, publish --check при затрагивании PAC, и проверки linker
placement/async stack frames при переносе target runtime. Даже перестановка
модулей без изменения алгоритма может изменить symbols и post-LTO layout.
Подробные команды и границы изменения — в [плане работ](DRIVER_STRUCTURE_PLAN.md).
