# Symbol lineage

`oer-symbol-lineage` carries source function names from an early vendor
archive to a later one whose functions carry generated names. It reads static
archives of RISC-V relocatable objects, pairs the functions of every adjacent
revision and composes those pairs from the oldest revision to the newest.

```console
cargo run --release -p oer-symbol-lineage -- \
  --repo ../esp32s31-bt-lib --library libbtdm_common.a \
  --obfuscated-prefix r_sym_ \
  --source https://github.com/espressif/esp32s31-bt-lib \
  --out target/symbol-lineage/libbtdm_common.json \
  --names target/symbol-lineage/libbtdm_common.toml \
  --redefine-syms target/symbol-lineage/libbtdm_common.syms
```

With `--repo`, revisions are every first-parent commit that changes the
library, oldest first, or the commits given by repeated `--revision`. A commit
that leaves the archive bytes unchanged is skipped. Without a repository,
repeated `--archive` paths give the revisions in order.

## Pairing

For each adjacent pair, evidence is applied from strongest to weakest. Every
step pairs only functions that are still unpaired on both sides:

1. the same symbol name, unique in both revisions;
2. the same normalized body, unique among the unpaired functions;
3. the call graph: two paired functions with the same number of calls vote
   for the callees at equal call positions. A pair needs a single, mutual
   vote and at least `--call-graph-minimum-ppm` body similarity;
4. body similarity of at least `--minimum-similarity-ppm`, with a
   `--similarity-margin-ppm` lead over the next candidate in both directions,
   followed by another call-graph pass.

A normalized body masks exactly the immediate bits that each RISC-V
relocation resolves at link time. Opcodes, registers, relocation types and
branch targets inside the function stay significant; target symbol names do
not. Similarity is the longest common subsequence of 16-bit parcels relative
to both lengths.

Ambiguity is never resolved by choice. Duplicate names or bodies and
competing votes leave functions unpaired. A function that first appears after
the named revision, or whose chain breaks, has no recovered name and needs a
manual decision.

A name is generated when it starts with an `--obfuscated-prefix`. Without a
prefix, a final `_`-separated component of at least sixteen alphanumeric
characters, containing lower case and either a digit or upper case in at
least a quarter of its characters, is treated as generated. Prefer the prefix
when the vendor scheme has one: the heuristic can misclassify a long camel-case
identifier.

## Outputs

- `--out`: JSON report with revision identities, per-step counts and, for
  every function of the last revision, its source name, origin revision and
  the evidence and body similarity of every step;
- `--names`: compact TOML map from each recovered generated name to its source
  name, origin revision, weakest evidence and lowest body similarity;
- `--redefine-syms`: `generated source` lines for
  `llvm-objcopy --redefine-syms`, which renames the symbols of the last archive
  for disassembly.

The standard output lists each step's counts: pairs by evidence, changed
bodies, pairs below half similarity and unpaired functions on each side.
