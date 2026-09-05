# Исправление сбоев диагностических HIL образов

Оба описанных ниже дефекта исправлены 2026-09-05. Образы
`diagnostic-rx-delivery` и `diagnostic-tx-architecture` прошли сборку,
placement, stack и autonomous source graph gates. Исходная диагностика
сохранена ниже для сравнения; аппаратные запуски не выполнялись.

Дата проверки: 2026-09-05. При миграции недрайверной структуры собраны все
12 image classes. Десять прошли сборку и проверки placement, stack и autonomous
source graph. Два приведённых ниже сбоя воспроизведены также в чистом detached
checkout исходного коммита `994aa0932e96f2a0a41fc1677f7bbf0a50008dda`.
Это были дефекты диагностических профилей, не регрессии переноса.
Аппаратные запуски не выполнялись.

## diagnostic-rx-delivery

Компилятор сообщает `E0004`: обработчик
[HilConnectedRxObserver::dropped](../../hil/targets/esp32s31/runtime/src/product_hil/rx_qualification.rs:169)
не покрывает `RxEnqueueError::PoolExhausted` и `RxEnqueueError::LinkDown`.
Обработчик и определение enum в
[network/interface](../../driver/network/interface/src/lib.rs) побайтно
совпадают с исходным коммитом. Одинаковый сбой получен настоящей сборкой
образа в обоих checkout, а не только сравнением исходников.

Исправление требует определить смысл этих причин в RX telemetry и согласовать
его с host analysis. Подмена обеих причин существующим `QueueFull` исказит
свидетельства; wildcard, который скрывает новые варианты enum, также не решает
проблему. Схема wire и счётчики в структурной миграции не менялись.

## diagnostic-tx-architecture

Linker отвергает образ: `runtime SRAM owners overlap the 8-KiB bootstrap
handoff margin`. Действует существующий
[ASSERT](../../hil/targets/esp32s31/linker/runtime/sections.x:299).
Оба linker scripts побайтно совпадают с исходным коммитом.

Повторная линковка baseline с единственным дополнительным `-Map` сохранила
ошибку и показала точную раскладку:

| Значение | Результат |
| --- | ---: |
| Конец INTERNAL_LOW | `0x2f07afc0` |
| Предельно допустимый конец владельцев SRAM | `0x2f078fc0` |
| Фактический `__runtime_dma_bss_end` | `0x2f07a200` |
| Требуемый bootstrap handoff | 8192 B |
| Фактически свободно | 3520 B |
| Дефицит | **4672 B** |

Крупные владельцы SRAM в baseline map:

| Владелец | Размер |
| --- | ---: |
| Wi-Fi supervisor `WIFI_MEMORY` | 179856 B |
| Network TX pool | 115904 B |
| GDMA memory probe destination | 49152 B |
| CPU0 IRQ stack | 32768 B |
| CPU1 IRQ stack | 32768 B |

Эта таблица локализует расход памяти, но сама по себе не разрешает уменьшать
пулы или переносить DMA buffers в PSRAM. Исправление должно проверить состав
диагностической прошивки и время жизни владельцев, затем подтвердить свойства
DMA и bootstrap handoff. Запас памяти и ASSERT не ослаблялись; сериализация
сборки не меняет эту раскладку.

## Воспроизведение и результаты

Из корня выбранного checkout, с установленным repository toolchain:

```console
cargo build --locked --offline -p open-esp-radio-hil-runner
env -u ESP_HAL_ROOT target/debug/open-esp-radio-hil-runner image build diagnostic-rx-delivery
env -u ESP_HAL_ROOT target/debug/open-esp-radio-hil-runner image build diagnostic-tx-architecture
```

Для сравнения использованы независимые Cargo output directories и обычный
параллелизм Cargo. Обе команды image build завершились кодом 1, вложенная
Cargo build — 101. Локальные логи и proof records находятся в
`~/.cache/oer-tooling-migration/`: `baseline-image-failures.json`,
`baseline-tx-layout.json`, `baseline-tx-architecture.map`,
`baseline-rx-delivery.log`, `baseline-tx-architecture.log` и
`hil-images/final/all-12-results.json`.

Общий source-only gate проверяет performance image и не подменяет полную
матрицу image classes.

## Исправление RX telemetry

Добавлены отдельные `PoolExhausted` и `LinkDown` в HIL drop reasons и счётчики
`network_pool_exhausted` / `network_link_down` в evidence. Runtime сохраняет
точную причину, tracker учитывает потерю только на post-reorder frontier,
не добавляя её в enqueue/consumer ledger. Host показывает обе причины и
отклоняет exact-delivery даже при совпадающих data cardinalities, например
когда потерян контрольный маркер.

Wire protocol повышен с 76 до **77**: firmware и runner требуется обновлять
вместе. Старые кадры отвергаются существующей проверкой версии; исторические
свидетельства не переписываются и не дополняются вымышленными нулевыми полями.
Максимальный RX evidence по-прежнему помещается в прежний размер кадра.

Регрессии проверяют причины потерь, отсутствие их в consumer ledger, сброс
между сессиями и отказ host assessment при каждом новом счётчике. Существующий
maximum-size round-trip включает максимальные значения обоих новых полей.

## Исправление SRAM ownership

Из общей DMA-арены выделен `Esp32s31DefaultScanMemory`: программная таблица
результатов сканирования не передаётся аппаратным DMA masters. Supervisor
размещает её в обычной статической памяти — PSRAM для этих image profiles.
RX/TX buffers, descriptors, beacon/frame storage и GDMA destination остались
в SRAM; их ёмкости не менялись.

`Esp32s31DefaultWifiMemory::claim` теперь принимает ссылку на scan owner.
Оба владельца резервируются до извлечения каких-либо cells. Если scan owner
занят, резервирование radio owner отменяется; существующие leases остаются
эксклюзивными. Проверены оба направления конфликта и успешное получение
свободного владельца после неудачной пары. Этот API sizing/ownership profile
изменился; публичная функция создания радио продолжает собирать ресурсы сама.

Фактическая раскладка исправленного TX ELF:

| Значение | Результат |
| --- | ---: |
| Scan owner в PSRAM | `0x50521790`, 10900 B |
| GDMA destination в SRAM | `0x2f025700`, 49152 B |
| `__runtime_dma_bss_end` | `0x2f077740` |
| Свободно до конца INTERNAL_LOW | **14464 B** |
| Требуемый bootstrap handoff | **8192 B**, прежний ASSERT |
| Дополнительный запас | **6272 B** |

Снижение занятого диапазона SRAM составляет 10944 B с учётом выравнивания
секции. Это исправление размещения CPU-only данных, не уменьшение бюджета
проверки или изменение диагностической нагрузки. Числа относятся к указанной
сборке, а не являются новыми фиксированными адресами/константами драйвера.

Логи исправления: `~/.cache/oer-diagnostic-fixes/`, в том числе
`rx-delivery-image.log`, `tx-architecture-image.log`, `tx-layout.json`,
`host-tests.log`, `telemetry-tests.log`, `memory-tests.log`.
Образы собраны параллельно с обычным параллелизмом Cargo. Сборка и статические
проверки не заменяют аппаратную qualification.

Проверки исправления:

- `cargo test --workspace --locked --offline`: 3957 passed, 23 ignored.
- Отдельный telemetry workspace: 23 passed; integration resource tests: 4 passed.
- Clippy затронутых host/protocol/telemetry/resource packages с `-D warnings`: PASS.
- Форматирование всех затронутых workspace и strict rustdoc resource API: PASS.
- Полный `tools/repo/audit-source-only.sh`, включая performance image и final ELF audit: PASS.
- Независимое ревью владения, RX evidence и читаемости: замечаний нет.

Полная повторная матрица завершена: **12/12 image classes PASS**. Для каждого
образа проверены placement, stack, autonomous source graph и соответствие
фактического application SHA-256 отчёту сборки. Агрегат:
`~/.cache/oer-diagnostic-fixes/all-12-images.json`.
