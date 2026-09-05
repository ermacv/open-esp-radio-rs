# Данные аудита driver

Снимок tracked tree на `134a75ac6f0eeeb60a76fca22d0bfbf51b1f4013`, 2026-09-04.
Основной документ: [аудит](../DRIVER_STRUCTURE_AUDIT.md);
результат: [завершённая миграция](../DRIVER_STRUCTURE_PLAN.md);
ход работы: [история](migration-history.md).

[Текущее дерево после структурной миграции](current-tree.txt) дополняет
исходный снимок ниже; оно отражает новые пути и отдельные тестовые модули.
Карта второй волны — [namespace-moves.csv](namespace-moves.csv): реальные
перемещения модулей, их дочерних файлов и выделения владельцев на 2026-09-05.
Это дополнение к исходному container mapping, а не новая версия старого снимка.
Третья и четвёртая волны — [responsibility-moves.csv](responsibility-moves.csv):
HCI/SoC разделение, ISR, chip RX frontier/transaction, профили capabilities,
STA role policy и affine publication/observation ports.
Пятая волна — [container-moves.csv](container-moves.csv): унификация семейства
`ieee80211`, security/WPA2 и Bluetooth LE LL. Карты применяются последовательно;
пути в старых снимках обозначают состояние соответствующей волны.
Шестая волна дополняет responsibility map: shared physical/role-transition
owners, AP observation, chip startup и connected GTK/replay transaction.
Седьмая волна — product resource profile в integration и PHY time bindings
в предметных модулях существующих Embassy adapters, вместе с их тестами.
Восьмая волна дополняет [container-moves.csv](container-moves.csv): пять
существующих крейтов памяти и сети сгруппированы по предметным областям.
Десятая волна дополняет обе карты: concrete Wi-Fi/Bluetooth Embassy runtime
перенесён в `driver/runtime/embassy/esp32s31/`; AP TX и Bluetooth system
разделены по обязанностям внутри прежних владельцев. Девятая волна исправляла
Bluetooth Clippy без перемещения контейнеров.
Одиннадцатая волна дополняет responsibility map: MAC parsing/state,
QoS/WMM, ESP-NOW codec/peer/security, HCI wire/controller/transport и
IEEE 802.15.4 mac/radio modules. Cargo containers и package identities прежние.
Финальный этап дополняет responsibility map: явный AP advertisement и chip
profile, AP service/engine, STA codecs, WPA2 и Bluetooth LE hierarchy.
Приватные PAC/HAL aliases удалены; происхождение generated и upstream кода
уточнено в PAC/adapter README. Текущее дерево отражает эти конечные пути.

- [tree.txt](tree.txt): все 767 файлов и 148 каталогов с отслеживаемыми потомками.
- [files.csv](files.csv): одна строка на файл; текущий owning package определяется
  ближайшим родительским Cargo.toml. Вложенные crates учитываются отдельно.
- [crates.md](crates.md): все 44 Cargo packages, размеры и предлагаемые корни.
- [dependencies.csv](dependencies.csv): все 256 dependency declarations из
  manifests, включая optional, dev, build и target-specific таблицы.

Файлы выбраны через `git ls-files driver`. Игнорируемые build outputs не
учитываются. Строки — число `splitlines()` исходного UTF-8 текста, включая
комментарии, пустые строки и тесты. Generated classification явно назначена
двум output-файлам: raw `src/lib.rs` и restricted PAC `src/generated.rs`.
Sidecars raw PAC помечены отдельно как handwritten.

`inline_test_modules` считает текстовые объявления `mod tests {`, допускающие
пробельные отступы; это не AST inventory всех cfg suites. `test_attributes`
считает буквальный `#[test]`. Doctests, compile-fail contracts и реально
выбранные Cargo profiles требуют отдельного compiled test discovery.

`responsibility` — крупная классификация текущего контейнера, а не утверждение,
что каждый файл внутри уже соблюдает границу. Исключения разобраны в основном
аудите. `proposed_container_path` — карта первого переноса контейнеров;
внутреннее разбиение функций/типов следует плану и не закодировано как
фиктивное соответствие «один старый файл — один окончательный новый файл».

Dependency CSV — объединение объявлений, не resolved production graph.
`target` сохраняет исходное условие, `optional` — значение manifest;
`local_path` разрешён относительно корня репозитория. Внешние registry
зависимости определяются package/alias; версии и включённые features нужно
смотреть в Cargo.toml и locked resolved metadata соответствующего workspace.

Проверено: точное совпадение множества файлов с tracked tree, размеры,
ближайшие owning manifests, уникальность предлагаемых путей назначения,
существование всех local dependency manifests и локальных ссылок документов.
Resolved root/owned/compat metadata также успешно проверены locked/offline;
результаты и ограничения проверки изложены в основном аудите.

Это датированный review artifact, а не новый нормативный генератор дерева.
После изменения базы инвентаризацию нужно обновить вместе с выводами;
после завершения миграции текущий контракт заменяет этот snapshot.
