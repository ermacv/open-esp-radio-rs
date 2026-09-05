# Технические имена и границы радиоподсистем

Конвенция на 2026-09-05. Текущее размещение показывает
[карта driver](../driver/README.md), решения и проверка миграции —
[итог структурирования](DRIVER_STRUCTURE_PLAN.md).
[Исторические находки](driver-audit/migration-history.md#исходные-находки-аудита-терминов-и-ход-их-реализации)
сохранены отдельно.

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

## Имена в завершённой структуре

- AP advertisement выбирается в chip `profile::ADVERTISEMENT`; portable
  `ap::profile::{Advertisement,LegacyRates,WmmParameters}` только представляет
  параметры и кодирует их. Сетевой QoS intent и hardware TX admission остаются
  разными обязанностями.
- WPA2: `crypto`, `eapol`, `frames/{security_ies,key_data,transmit}` и
  `state/{supplicant,authenticator}`. Ключи, производные секреты и zeroization
  остаются вместе с владельцем; кадр не создаёт право установки ключа.
- STA codecs: `station/{association,management,security,data}`; A-MSDU внутри
  `data/amsdu`, sequence owners в `sequence`. Закрытый capability encoder
  расположен вместе с association encoder, которому он нужен.
- Chip Bluetooth: `le/{dtm,advertising,scanning,peripheral}`. Shared controller,
  IRQ и scheduler остаются вне role-specific LE namespaces. DTM event/scheduler
  modules и connectable recurrence используют разделители пути вместо
  повторяющихся префиксов файлов. Каждая role сохраняет свой lifecycle owner.

Внутренние ссылки PAC/HAL используют реальные доменные пути; временные
crate-private flattened aliases удалены. Публичные reexports сохраняют
поддержанный API и не создают дополнительных владельцев. Их наличие не
означает вторую реализацию или незавершённое перемещение.

`use open_esp_radio as oer;` задаёт короткое имя facade без изменения package
identity. Примеры используют `oer::wifi` и контекстные локальные имена
`RadioConfig`, `RadioSystem`, `NetworkRunner` и `StackResources`. Полные
chip/protocol prefixes у внешних root exports продолжают различать типы
разных backend; состояния `Prepared`, `Running`, `Quarantined` и точные
failure owners не сокращаются до неоднозначных `State`/`Error`.

Переименование всех packages в `oer-*` и объединение crates не требуются для
этой структуры. Границы зависимостей и приватности важнее числа crates.
Vendor/upstream register names, SVD identifiers и Cargo revisions сохраняют
своё происхождение; они не переименовываются ради визуальной симметрии.
