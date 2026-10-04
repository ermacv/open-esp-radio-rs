# oer-mir-facts

A `RUSTC_WRAPPER` that compiles every crate exactly as the real compiler does
and, for each crate built for the firmware target, records the indirect-call
facts its MIR states. The stack analysis (`tools/riscv/stack`) resolves the
indirect sites of an image's machine code with them, without anything in the
image changing.

It links the toolchain's `rustc_driver` through `rustc_private`, so it is a
workspace of its own, built with `RUSTC_BOOTSTRAP=1` (its `.cargo/config.toml`)
and run with the toolchain's `lib/` on `LD_LIBRARY_PATH`. It needs the pinned
toolchain's `rustc-dev` component, about 1.6 GB installed, which
`rust-toolchain.toml` leaves out so that no other checkout or CI job pays
for it: `rustup component add rustc-dev` where the driver runs. A toolchain update can change the `rustc_public` API it
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

- `calls`: each instance with indirect calls, by its function key: a call through a function pointer of a type
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
- `signatures`: the function-pointer type each vtable function is called as
  (a drop glue's `fn(&mut T)`, a method's signature with its `self`), so
  that a leaked trait's functions reach only the sites their ABI fits.
- `leaked_types`: function-pointer types whose values leave the type: cast
  to a pointer or an address (`f as *const ()`, an `AtomicPtr<()>`),
  transmuted to anything but another function-pointer type, a union's field,
  or behind a pointer cast to another pointee (`ptr::write` into bytes). A
  pointer cast reaches one memory as both types, so both sides leak. Every
  function made a pointer of a leaked type is a candidate of every site whose
  ABI it fits: a function leaves the candidates of other types' sites only
  when no address-taking of its type loses the type.
- `leaked_functions`: functions a constant holds where the type at that
  offset is no function-pointer type (an address, an erased pointer, a field
  the layout does not single out), by the key of their own signature's
  pointer type; they leak like a leaked type's functions.
- `edges`: transmutes between function-pointer types, each target type's
  source types: a site of the target type also reaches the sources'
  functions. A transmute that changes only lifetimes keys both sides alike
  and records nothing.
- `leaked_traits`: principal traits of `dyn` values a leak carries: every
  function of their vtables leaks, and so does what their implementors carry.
- `trait_contents`: for each trait, what the types made a `dyn` of it carry
  (`keys`: function-pointer types, `traits`: nested `dyn` traits, `unknown`),
  for the leak's closure over every crate.
- `unknown_leak`: a reinterpreted value carries a part whose contents the
  driver cannot enumerate (a `dyn` without a principal trait, an opaque
  type): every function made a pointer leaks.

A reinterpretation between two views of one shape leaks nothing: a
`#[repr(transparent)]` wrapper (`UnsafeCell`, `MaybeUninit`, `NonNull`,
`Pin`) views memory as its one field of non-zero size does, every reference
and raw pointer is one pointer kind, and a pointer to a slice or an array
views memory as a pointer to its element. A union reinterprets only when its
fields of non-zero size have more than one shape (`MaybeUninit` does not).

A function is keyed by its symbol demangled without the crate hashes a
compilation gives it (`core::fmt::write`, `<sample::A as sample::Speak>::speak`),
so that the precompiled `core` matches the facts of `core` compiled apart from
the toolchain's sources; two functions of one path only unite their facts. A
function-pointer type is keyed by its ABI, inputs and output as rustc prints
them, without lifetimes, binders or `unsafe`, which codegen erases.

The walk starts at every monomorphic item of the crate and its statics, and
follows direct calls, coercions and constants to every instance with a body,
generic instances included. A precompiled crate (`core` from the toolchain)
gives facts only for the generic code another crate instantiates; its own
functions' indirect sites stay holes, and its own reinterpretations are not
seen: a precompiled crate's leaks come from compiling it apart.

A constant's function pointer is typed by the field at its offset in the
constant's type, through structs, tuples, closures, arrays and enum variants
that agree; a pointer field's memory takes the field's pointee type.
