A station needs at least one ticket, small buffer and group-key slot. The
same constructor rejects each zero-sized pool at compile time; eight remains
the maximum per pool. The minimum also applies to an open-mode composition,
so its fixed resource layout is valid when constructed for protected mode.

No ticket:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _) -> _ = construct::<0, 1, 1>;
core::hint::black_box(constructor);
}
```

No buffer:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _) -> _ = construct::<1, 0, 1>;
core::hint::black_box(constructor);
}
```

No group slot:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _) -> _ = construct::<1, 1, 0>;
core::hint::black_box(constructor);
}
```

Too many tickets:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _) -> _ = construct::<9, 1, 1>;
core::hint::black_box(constructor);
}
```

The compiling twin: one owner of each kind:

```
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _) -> _ = construct::<1, 1, 1>;
core::hint::black_box(constructor);
}
```
