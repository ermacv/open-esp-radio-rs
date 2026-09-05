# Итог структурирования driver

Проверки ниже относятся к завершённой структурной миграции. Текущий запуск
source gate — `cargo xtask check source-only`; результаты прежнего shell gate
не подменяют проверку новой автоматизации.

Дата: 2026-09-05. Структурная миграция и итоговые проверки завершены. Область работы — организация существующего кода и владения,
без расширения поддержанных радиорежимов или hardware qualification.

Нормативная карта — [driver/README.md](../driver/README.md), правила имён —
[protocol naming](DRIVER_PROTOCOL_NAMING.md), происхождение generated кода —
[PAC](../driver/chips/esp32s31/pac/README.md). Исходный план и результаты волн
сохранены как [история миграции](driver-audit/migration-history.md).

## Закрытые границы

| Область | Итог |
| --- | --- |
| Контейнеры | Portable protocols, chip backends, memory/network, внешние adapters, radio runtime и final integration разделены; generic `common` удалён |
| Тесты | Unit suites вынесены в отдельные child files; compile-fail contracts остаются рядом с типами; production код не переносился в probes |
| PAC и upstream | Два generated Rust outputs отделены от handwritten ownership/sidecars; publisher остаётся в tooling, esp-hal/esp-pacs bindings — в adapters |
| Hardware / executor | RX frontier/transaction, startup и association security owners находятся в chip; waits, endpoints и wake bindings — у executor; budgets/static claims — в integration |
| IEEE 802.11 | Framing/state, QoS/WMM, ESP-NOW codec/peers/security разделены; AP advertisement задаёт chip profile; STA codecs сгруппированы по назначению |
| WPA2 и AP | Crypto/zeroization, EAPOL, frame codecs и handshake owners разделены; AP service и engine сохраняют по одному owner при отдельных peer/security/TX/RX/power-save modules |
| Bluetooth | HCI wire/transport/controller отделены; chip LE имеет dtm/advertising/scanning/peripheral namespaces; shared controller/IRQ/scheduler и целые actor loops сохранены |
| IEEE 802.15.4 | MAC frame codec отделён от radio command/event/state/channel/capability; platform ISR binding не подменяет owner |
| Имена | 41 приватный flattened alias PAC/HAL удалён; 45 Bluetooth modules используют иерархию пути; примеры используют `oer` и контекстные импорты |
| Проверки | Ownership tests, Cargo graph policies, compiler lints, generated-source validation и final-image checks; Rust source-spelling assertions удалены |

## Доказательства финального этапа

AP profile comparison проверяет 5976 случаев на скомпилированном старом и
новом коде: совпадают результаты, полные выходные буферы и parser observations.
Переносимые encoders принимают explicit `Advertisement`; единственный внешний
runtime consumer обновлён. Это намеренное изменение codec API, а не смена
advertised capabilities.

Независимые reviews подтвердили сохранение владельцев, полей, тел методов,
zeroization и порядка handoff/drop. Bluetooth: 2716 items, 3007 imports,
73 public export statements и effective cfg в восьми profiles сохранены.
WPA2/STA: сохранены прежние exports и production/test items. AP service/engine:
248 definitions; локальная доступность двух security helpers остаётся внутри
прежнего service owner. Тесты не удалялись ради зелёного результата.

Финальный прогон 2026-09-05, `TMPDIR=/var/tmp`, Cargo locked/offline,
обычная параллельность Cargo и test harness:

| Проверка | Результат |
| --- | --- |
| `cargo check --workspace` | PASS |
| `cargo test --workspace` | 3942 passed, 0 failed, 23 ignored; два новых AP profile tests, прежние suites сохранены |
| `cargo fmt --all -- --check` и шесть отдельных compositions | PASS |
| `tools/repo/audit-source-only.sh` | PASS: strict host Clippy, 22 tool tests, Cargo boundaries, target/source/PAC/qualification gates |
| Bluetooth strict host/target Clippy, default/all-feature PAC/HAL и AP target checks | PASS |
| Strict rustdoc MAC/WPA2/AP/Bluetooth | PASS |
| HIL images: performance, correctness, diagnostic-core0-rx-coarse, diagnostic-core0-rx-cycles, diagnostic-task-poll | PASS: placement, stack, source graph; SHA-256 сверен с фактическими images |
| Bluetooth example, actual target release build | PASS |
| 90 manifests/locks и два generated PAC outputs | Без изменений в финальном этапе |

Сборки некоторых Wi-Fi image profiles сохраняют два `dead_code` warning:
`deferred_shared_rx_admission_for_diagnostics` и
`record_protocol_publication`. Они не подавлялись и не относятся к Bluetooth
Clippy; этот отчёт не заявляет отсутствие warnings во всех feature profiles.
Оборудование не прошивалось; host/image PASS не заменяет датированные HIL
измерения. Детальные логи и сравнения находятся локально в
`~/.cache/oer-driver-final/`.

## Принятые решения

- Сохранены 44 production packages: новые crates не требуются. Объединение
  portable STA/AP/SoftMAC и chip facades не даёт доказанного выигрыша поверх
  действующих dependency/privacy boundaries.
- Package identities `open-esp-radio-*` и pinned revisions сохранены. `oer`
  применяется как локальный alias; полный package rename не является долгом
  этой миграции. Публичные compatibility exports сохраняют внешний контракт.
- Большие связанные PHY algorithms, DMA ring owner и цельные controller
  actor loops сохраняются вместе. Размер файла не является основанием
  разрывать владение или менять async cancellation points.
- Существующие lifecycle defects, CACHE coordination, новые Bluetooth/802.15.4
  возможности, fairness и hardware qualification требуют отдельных задач с
  behavioral/HIL evidence. Структурная миграция их не скрывает и не заявляет
  реализованными.

[Текущее дерево](driver-audit/current-tree.txt) и
[карта перемещений](driver-audit/responsibility-moves.csv) отражают итоговые пути.
[Shell audit](SHELL_SCRIPT_AUDIT.md) отдельно фиксирует устранённые host lock
сбои и замену ненадёжных проверок; сериализация тестов не является исправлением.
