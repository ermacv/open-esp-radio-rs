# oer-mir-facts

A `RUSTC_WRAPPER` that compiles every crate exactly as the real compiler does
and, for each crate built for the firmware target, records the indirect-call
facts its MIR states. The stack analysis (`tools/riscv/stack`) resolves the
indirect sites of an image's machine code with them, without anything in the
image changing.

It links the toolchain's `rustc_driver` through `rustc_private` (the
`rustc-dev` component of the pinned toolchain), so it is a workspace of its
own, built with `RUSTC_BOOTSTRAP=1` and run with the toolchain's `lib/` on
`LD_LIBRARY_PATH`. A toolchain update can change the `rustc_public` API it
uses; the driver changes with it.

## Running it

```console
RUSTC_WRAPPER=tools/mir-facts/target/debug/oer-mir-facts \
OER_MIR_FACTS_DIR=<facts directory> \
OER_MIR_FACTS_TARGET=riscv32imafc-unknown-none-elf \
cargo build ...
```

A crate compiled for `OER_MIR_FACTS_TARGET` runs through the in-process
compiler with a callback that only reads the analysed crate; every other
invocation (build scripts, procedural macros, `--print`, `-vV`) goes to the
real `rustc` untouched. Without `OER_MIR_FACTS_DIR` the wrapper only compiles.

## Facts

Each crate writes `<crate>-<hash>.json`:

- `calls`: each instance with indirect calls, by its mangled symbol (as the
  ELF names it): a call through a function pointer of a type
  (`fn_pointer`), or through a `dyn` vtable (`dyn`: the principal trait and
  the vtable entry index, the header's drop, size and align being 0, 1, 2),
  or `unknown` where the MIR names no target type (a `dyn` without a
  principal trait, a call the compiler cannot resolve there): such an
  instance's indirect sites stay holes.
- `fn_pointers`: the functions made function pointers of each type, from
  `ReifyFnPointer` and `ClosureFnPointer` coercions and from the function
  pointers constants and statics hold.
- `vtables`: each trait's vtable entries, by index, from every unsizing
  coercion to a `dyn` and every vtable a constant holds; entry 0 is the
  type's drop glue.
- `polluted`: function-pointer types a transmute produces from another type:
  a call through one may reach any function.

A function-pointer type is keyed by its ABI, inputs and output as rustc
prints them, without lifetimes, binders or `unsafe`, which codegen erases.

The walk starts at every monomorphic item of the crate and its statics, and
follows direct calls, coercions and constants to every instance with a body,
generic instances included. A precompiled crate (`core` from the toolchain)
gives facts only for the generic code another crate instantiates; its own
functions' indirect sites stay holes. Memory reinterpretations other than a
transmute (a union field, a pointer read through another type) are not seen.
