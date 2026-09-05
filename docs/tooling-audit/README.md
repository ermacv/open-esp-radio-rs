# Инвентаризация недрайверной структуры

База исходного снимка `994aa093`, 2026-09-05. Текущая архитектура и результат:
[REPOSITORY_TOOLING_ARCHITECTURE.md](../REPOSITORY_TOOLING_ARCHITECTURE.md).

- [initial-audit.md](initial-audit.md): исходные выводы и принятый план.
- [current-tree.txt](current-tree.txt): итоговое дерево текущих исходников.
- [responsibility-moves.csv](responsibility-moves.csv): последовательные переносы и разделение модулей; старые paths относятся к соответствующим этапам.
- [FIRMWARE_SUPPORT_REVIEW.md](FIRMWARE_SUPPORT_REVIEW.md): решение по общей загрузочной инфраструктуре.
- [DIAGNOSTIC_PROFILE_FAILURES.md](DIAGNOSTIC_PROFILE_FAILURES.md): причины и исправления двух сбоев HIL image classes, сравнение с исходным коммитом.
- [XTASK_PORTABILITY_PLAN.md](XTASK_PORTABILITY_PLAN.md): перенос repository automation из shell/Python в Rust; Linux/OpenWrt стенд сохраняет отдельную ответственность, другие host OS вне текущего этапа.
- [tree.txt](tree.txt): все 1228 tracked файлов выбранных шести областей.
- [files.csv](files.csv): размеры, текущая крупная ответственность и ближайший owning package.
- [packages.csv](packages.csv): все 35 packages и ближайшие declared workspace roots.
- [dependencies.csv](dependencies.csv): 247 package dependency declarations, включая dev/build/target-specific.

Области: tools, verification, hil, qualification, svd, examples; 233 каталога
с tracked потомками. Документы этого нового аудита, driver, ignored/private
inputs и build outputs не входят в snapshot. Файлы выбирались `git ls-files`,
Cargo manifests разбирались TOML parser. Каждый прямой local path проверен.

Dependency CSV отражает объявления packages, не resolved feature graph.
`workspace = true` требует таблицы соответствующего root manifest; пустой
local_manifest у такой строки не означает registry-only зависимость.
Workspace dependencies/patches из virtual manifests не выдаются за package
edges. Три основных Cargo roots отдельно проверены locked/offline metadata.

Размеры — bytes и физические строки, включая комментарии и тесты. Для
сжатых/бинарных tracked fixtures число строк не назначено. Это инвентаризация
пути, не разрешение публиковать приватные vendor artifacts. `file_kind` —
грубый классификатор; reviewed platform SVD не объявлен generated output.
`current_responsibility` описывает область, а не доказывает корректность
каждой функции. Рекомендованные перемещения изложены отдельно в аудите;
исторический snapshot не должен притворяться уже изменённым деревом.
