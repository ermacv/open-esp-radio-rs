# oer-interrupt-table

Each image's static table of its peripheral interrupt sources: for every
source its handler, its level and its core, declared once per image with
`interrupt_table!` (a chip's platform wraps it with its types, such as
`oer_esp32s31_platform_runtime::interrupt_table!`).

The table says where a source may be routed; the source's owner decides when.
For each entry the macro defines:

- the source's strong handler symbol, named as the source, which the chip's
  vector slot of the source links to: a second handler for one source fails
  the link;
- `__oer_interrupt_body_<SOURCE>`, a word holding the handler function's
  address, for the tools that check where the handler lies;
- a token type, neither `Copy` nor `Clone`, which `Interrupts::take` hands out
  once per image. `enable` and `disable` take only a token; `enable` routes
  the source to the table's level on the table's core and fails on another
  core, or for a token whose source the table does not list so. `Entry` is an
  `unsafe` trait and the token's constructor an `unsafe fn`: only the macro
  creates a token;
- with `#[cfg(...)]` before its doc comment, nothing at all in a build that
  leaves the entry out: its table element has no handler (a zero word).

The table is `INTERRUPT_TABLE`, a slice of `#[repr(C)]` bindings exported as
`__OER_INTERRUPT_TABLE` for the tools that read the image; the image hands it
to its platform's runtime as a typed value.

A `Route` is a token with its type erased: only consuming a token creates
one, it is neither `Copy` nor `Clone`, and `enable_route`/`disable_route`
check it as `enable`/`disable` check the token; an owner keeps its sources'
routes behind a concrete type. `Adopted` holds the image's table for every
core: adopted once (a second adoption is an error), absent before.

`install` silences every source of the current core's entries and checks each
vector slot against the table; `verify` checks again that every slot holds its
handler and every source is silent or routed to its level. `Matrix` is the
chip's interrupt matrix as the table drives it; `__fake_matrix!` is a host
model for tests.

```console
cargo test -p oer-interrupt-table
```
