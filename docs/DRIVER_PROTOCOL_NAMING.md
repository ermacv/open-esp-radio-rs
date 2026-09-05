# Технические имена и границы радиоподсистем

Анализ и перенос контейнеров: 2026-09-05. Семейства каталогов унифицированы;
внутренние разделения ответственности ниже остаются следующими шагами.
Текущее размещение показывает
[карта driver](../driver/README.md), порядок миграции —
[структурный план](DRIVER_STRUCTURE_PLAN.md).

## Решение

Использовать `ieee80211`, `ieee802154`, `bluetooth` как канонические семейства
на переносимом, chip и adapter уровнях. Здесь family namespace означает
реализацию семейства протоколов: внутри допустимы явно обозначенные extensions,
локальная политика и silicon backend. Он не обещает реализацию всего стандарта
или сертификацию.

`wifi` — допустимое имя технологии и удобного прикладного API. Существующее
имя не является технической ошибкой. Для внутренней навигации этого проекта
выбрана более точная конвенция стандартных семейств. `wlan` не добавляет здесь
нового владельца или слоя; третий синоним для тех же модулей не нужен.

Унификация относится к ответственности и направлению зависимостей. Она не
требует одинаковых MAC/LL/HCI подкаталогов у разных стандартов, общего
`RadioFrame`, одинаковых IRQ queues или ещё одного набора crates.

## Значение терминов

| Термин | Значение | Правило в проекте |
|---|---|---|
| IEEE 802.11 | Семейство спецификаций WLAN MAC и PHY | `ieee80211` для внутренней реализации семейства |
| WLAN | Wireless Local Area Network: категория беспроводной локальной сети | Термин для сети/интерфейса в документации; сам по себе не определяет protocol layer |
| Wi-Fi | Технология и экосистема на основе IEEE 802.11, с программами совместимости Wi-Fi Alliance | `wifi` допустим в facade и пользовательских настройках; название не заявляет Wi-Fi CERTIFIED |
| IEEE 802.15.4 | Семейство MAC/PHY для low-rate wireless networks | Сохранить `ieee802154`; конкретные channel/PHY/frame limits принадлежат реализованному профилю |
| WPAN | Wireless Personal Area Network: более широкая категория | Не использовать как новое имя конкретного 802.15.4 backend |
| Bluetooth | Семейство Bluetooth SIG, включающее BR/EDR и LE | Корень `bluetooth`; текущие LE процедуры обозначать явно через `le` |
| HCI | Граница Host↔Controller, команды, события и data transport | `bluetooth/hci`; это не эфирный протокол и не полный Host stack |
| LE Link Layer | Эфирные LE PDU и процедуры Link Layer | `bluetooth/le/ll` |

Область IEEE 802.11 определена как MAC/PHY для WLAN в
[описании рабочей группы](https://www.ieee802.org/11/abt80211.html).
Связь Wi-Fi с 802.11 и роль именования Wi-Fi Alliance описаны
[IEEE SA](https://standards.ieee.org/beyond-standards/the-evolution-of-wi-fi-technology-and-standards/).
Для 802.15.4 основание —
[IEEE 802.15.4-2024](https://standards.ieee.org/ieee/802.15.4/11041/).

Bluetooth LE Controller включает PHY и Link Layer; Host содержит свои верхние
протоколы, HCI связывает эти стороны. HCI также применяется к BR/EDR, поэтому
весь HCI нельзя назвать LE LL. Основание —
[Bluetooth Core Architecture](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-62/out/en/architecture%2C-change-history%2C-and-conventions/architecture.html).
`legacy advertising` в текущем коде означает legacy LE advertising.
Переименование Bluetooth в `ieee802151` было бы неверной ориентацией на старую
адаптацию: [IEEE 802.15.1-2005](https://standards.ieee.org/ieee/802.15.1/3513/)
имеет статус Inactive-Withdrawn.

IEEE 802.15.4 сам по себе не обозначает Thread или Zigbee. Эти стеки используют
его как основу и имеют дополнительные уровни:
[Thread Group](https://threadgroup.org/what-Is-thread/overview),
[Zigbee specification](https://csa-iot.org/wp-content/uploads/2023/04/05-3474-23-csg-zigbee-specification-compressed.pdf).
Каталоги под отсутствующие Thread/Zigbee/BR-EDR/Host реализации создавать не нужно.

## Одинаковые границы ответственности

| Граница | Разрешённая ответственность | Не следует выводить из имени |
|---|---|---|
| `frame`, `wire`, `ie` | Представление пакета, разбор и кодирование | Владение radio, peer lifecycle или исполнение команд |
| Protocol/MAC/LL state | Последовательности, окна, peer state, retries, security transitions | Наличие MMIO, executor или аппаратного offload |
| `interface`, `contract` | Возможности и контракты между владельцами | Собственный скрытый peer/key store |
| Chip backend | Дескрипторы, timed execution, аппаратный MAC/LL, interrupt semantics | Полная реализация верхнего стека |
| Adapter | Привязка внешней библиотеки, таймера, CPU route, очереди к существующим capabilities | Второй независимый hardware owner |
| Integration | Выбор ресурсов, размещения, окончательных bindings и композиции | Production алгоритмы, скопированные из драйвера |

`mac` в `driver/ieee80211` означает переносимые MAC-механизмы;
`mac` в `driver/chips/esp32s31/ieee80211` — аппаратное исполнение и его
контракты. Chip prefix уже обозначает реализацию конкретного оборудования.
Для Bluetooth сохраняется термин `ll` в его стандартном значении; заменять
его на `mac` ради визуальной симметрии не следует.

`softmac` описывает распределение MAC-работы между software и hardware,
а не отсутствие offloads. В качестве примера реального разделения полезна
[документация Linux mac80211](https://wireless.docs.kernel.org/en/latest/en/developers/documentation/mac80211.html).
В этом проекте источником истины остаётся пооперационный
`MacOperationOwnership`, а не сходство с Linux API.

## Конкретные находки в текущем коде

### IEEE 802.11 и расширения

1. [Текущий ieee80211 crate](../driver/ieee80211/mac/src/lib.rs) шире codec:
   `block_ack` владеет сессиями, `fragmentation` — reassembly,
   `station::StaTxSequenceCounters` — состоянием sequence spaces. Поэтому
   целевой container `ieee80211/mac` точнее, чем переименование всего crate
   в `frame` или `wire`. Внутри нужны frame/IE и state modules.
2. [ESP-NOW](../driver/ieee80211/mac/src/esp_now.rs) — расширение Espressif
   поверх vendor-specific Action frames, что прямо описано
   [Espressif](https://docs.espressif.com/projects/esp-idf/en/v5.5/esp32/api-reference/network/esp_now.html).
   Его codec и v2 reassembly следует обозначить
   `extensions/espressif/esp_now`. Они переносимы между чипами; vendor protocol
   не становится ESP32-S31 hardware только из-за своего происхождения.
3. [SoftMAC crate](../driver/ieee80211/softmac/src/lib.rs) содержит и контракт,
   и [ESP-NOW peer/protocol owners](../driver/ieee80211/softmac/src/esp_now.rs),
   и [key/replay owners](../driver/ieee80211/softmac/src/esp_now_security.rs).
   Нужны явные внутренние `contract` и extension protocol modules. Перенос
   только имени `softmac → interface` дал бы неточное обещание.
4. [WMM](../driver/ieee80211/mac/src/wmm.rs) объединяет vendor IE parser,
   общие UP/AC значения и admission/downgrade policy. Целевая граница:
   `qos` для общих понятий, `extensions/wmm` для формата WMM IE,
   policy у владельца STA/MAC. WMM не является общим именем всей QoS.
5. [WPA2 crate](../driver/ieee80211/security/wpa2/src/lib.rs) реализует конкретный
   WPA2-Personal subset. Сохранить `security/wpa2`; отдельно обозначить EAPOL
   framing, RSN element parsing, supplicant/authenticator и secret storage.
   Общее имя `rsn` для всего нынешнего crate скрыло бы фактический subset.
   `Wpa2Interface::{Station,AccessPoint}` описывает роли владельцев;
   названия cryptographic механизмов следует сохранять независимо от ролей.
6. [Portable AP encoders](../driver/ieee80211/mac/src/ap.rs) всё ещё содержат
   выбранные rates, capabilities и WMM response defaults. Вынос HT profile
   предыдущей волны не выделил все local profiles. Это дополнительная задача
   profile→encoder boundary, не автоматическое следствие нового пути.
7. `sta`/`ap` остаются допустимыми сокращениями ролей. В документации STA как
   клиент инфраструктурной сети следует уточнять как non-AP STA: термин STA
   стандарта не исключает AP. `WifiSecurityMode` с Open/Wpa2Personal точнее
   описывать как выбранный BSS security mode, а не полный каталог стандартной
   802.11 security.

### Bluetooth

[Portable LL](../driver/bluetooth/le/ll/src/lib.rs) уже отделён от HCI/MMIO и
содержит LE air protocol. Теперь он размещён в `bluetooth/le/ll`.

[HCI package](../driver/bluetooth/hci/src/lib.rs) владеет транспортом и
controller-side command plane. [Bootstrap](../driver/bluetooth/hci/src/bootstrap/state.rs)
хранит reset state; [controller](../driver/bluetooth/hci/src/controller.rs)
объединяет channel/configuration/readiness;
[order](../driver/bluetooth/hci/src/command/order.rs) удерживает affine
command/response authority. Это обоснованные обязанности HCI boundary.

Предлагаемая навигация внутри того же package:

```text
hci/src/
  wire/command, event          # только представление пакетов
  transport/in_process        # текущий транспорт без UART/H4 framing
  controller/resources, endpoint, order, bootstrap
  controller/le/advertising, scanning, dtm
```

Это карта responsibilities, а не предложение разнести закрытые части
одного command epoch по независимым crates. HCI command plane не является
полным LE Controller; аппаратный LL runner остаётся в chip/adapter слоях.
Bootstrap остаётся общим controller module: в нём есть Reset, event masks,
HostBufferSize и flow control. LE-specific команды и выбранные capabilities
следует обозначать внутри профиля, сохраняя единый reset/response owner.

Chip `bluetooth/{controller,interrupt,scheduler,memory}` сохраняет аппаратные
owners. LE-specific процедуры получают `bluetooth/le/{dtm,advertising,
scanning,peripheral}`. `host` обозначает настоящий Host stack; текущий
[Host-facing ExternalController](../driver/integration/esp32s31/embassy/bluetooth/src/system.rs)
— транспортный интерфейс к контроллеру. Trouble сейчас используется как
dev-dependency; из названия интерфейса не следует production GATT/L2CAP/SMP.

### IEEE 802.15.4 и реальные ISR owners

[Portable crate](../driver/ieee802154/src/lib.rs) содержит bounded MAC bytes,
metadata и finite radio command/event state. Его можно организовать как
`mac/frame` и `radio/{command,event,state,channel,capabilities}` внутри
существующего crate. Он не является полным MAC/Thread stack.

Chip `ieee802154/mac` планирует транзакции, `runtime` исполняет их через
закрытую capability, `irq` определяет sampled/acknowledged события.
Переименовывать executor-neutral `runtime` в Embassy неверно.

Найдена конкретная ошибка описания владения:
[esp-hal adapter](../driver/adapters/esp-hal/esp32s31/ieee802154/src/lib.rs)
сам хранит `INTERRUPT_REGISTERS` с PAC interrupt owner и публикует его перед
включением CPU route. [Embassy adapter](../driver/adapters/embassy/esp32s31/ieee802154/src/lib.rs)
хранит другую очередь — уже acknowledged event tokens. Заголовок esp-hal
adapter исправлен по фактическому коду; storage не переносился.

Эта очередь не взаимозаменяема с coalesced Wi-Fi wake: её переполнение
теряет уже подтверждённое аппаратное событие и требует failure path.
Единое имя `irq` не разрешает объединить эти разные протоколы handoff.

## Выполненное перемещение контейнеров

| До волны, относительно driver | Текущий путь | Сохранённая граница / следующий шаг |
|---|---|---|
| `wifi/ieee80211` | `ieee80211/mac` | Прежний package; затем внутреннее разделение frame/state/extensions |
| `wifi/softmac` | `ieee80211/softmac` | Явно описать contract и extension protocol owners |
| `wifi/{sta,ap,datapath}` | `ieee80211/{sta,ap,datapath}` | Сохранить DAG и role boundaries |
| `wifi/wpa2` | `ieee80211/security/wpa2` | Сохранить реализованный security subset |
| `chips/esp32s31/wifi` | `chips/esp32s31/ieee80211` | Включая существующие вложенные MAC/DMA/role crates |
| `adapters/embassy/wifi` | `adapters/embassy/ieee80211` | Только контейнер, без изменения async contracts |
| `adapters/{embassy,esp-hal}/esp32s31/wifi` | `adapters/{embassy,esp-hal}/esp32s31/ieee80211` | Compat остаётся отдельным endpoint/package |
| `adapters/embassy/esp32s31/wifi-compat` | `adapters/embassy/esp32s31/ieee80211-compat` | Не вложить compat в owned implementation |
| `integration/esp32s31/embassy/wifi` | `integration/esp32s31/embassy/ieee80211` | Прикладной Wi-Fi facade сохраняется |
| `bluetooth/ll` | `bluetooth/le/ll` | LE specificity; HCI остаётся sibling |
| `ieee802154` и соответствующие chip/adapters | Без переименования семейства | Уточнить внутренние modules и descriptions |

Новые Rust namespaces следуют тем же family names. Внутри понятного пути
предпочитать `mac::Frame`, `rx::Live`, `le::ll::Connection`,
`hci::controller::CommandReady` вместо повторения family/chip в каждом типе.
Ownership/state слова сохранять: `Ready`, `Active`, `Pending`, `Quarantined`
обозначают разные права, а не лишние приставки.

Shared `chips/esp32s31/phy`, HAL owner, PAC и coexistence остаются общими:
[PHY](../driver/chips/esp32s31/phy/src/lib.rs) обслуживает все три семейства.
Его нельзя целиком перенести в `ieee80211/phy` или назвать только `rf`: там
есть также calibration, baseband/timing и зарегистрированные clients.
Handwritten HAL semantic namespaces можно привести к family convention;
vendor hardware names (`WIFI_MAC`, `BTBB`), SVD/generated PAC names, upstream
`esp_hal::peripherals::WIFI` и исторические evidence identifiers сохраняют
своё происхождение. Их переименование не следует из этой терминологии.

## Порядок реализации и критерии

1. Принять словарь responsibilities; обновлять descriptions одновременно
   с реальным модульным выделением, не объявлять существующий owner codec.
2. Перенести перечисленные containers с прежними Cargo package identities.
   Обновить все manifests, isolated workspaces, tools и source bindings.
3. Разделять внутренние leaf responsibilities отдельными изменениями.
   Сначала wire/state внутри существующих crates; затем оценивать межкрейтовые
   переносы. ESP-NOW codec должен остаться ниже peer/security policy; низкий
   MAC crate не должен получить обратную зависимость на SoftMAC.
4. После стабилизации пространства имён сокращать типы и local imports;
   `oer` удобно как alias facade. Полный `oer-*` package rename — другая волна,
   с собственной картой symbols/consumers.
5. Проверять dependency direction, affine return/drop/stop contracts, весь
   test inventory, supported feature profiles, PAC reproducibility и target
   placement. Название семейства не является evidence новой возможности.

Выполнен этап 2: 13 контейнеров с 17 crates перенесены по этой карте.
Cargo package identities, Rust API, features и владельцы ресурсов сохранены.
Обновлены manifests всех workspace roots, active source bindings, stack policy
selectors, проверки и навигация. Исторические evidence paths сохраняют
исходное значение. Полная карта — [container-moves.csv](driver-audit/container-moves.csv).

После дополнительного boundary review приоритет уточнён в структурном плане:
сначала product resource profile и PHY time bindings, затем предметные
memory/network/runtime paths и внутренние frame/state/extensions, HCI и
802.15.4 modules. Сокращать имена после стабилизации этих границ.
Перенос каталогов не заявляет новую аппаратную qualification.
