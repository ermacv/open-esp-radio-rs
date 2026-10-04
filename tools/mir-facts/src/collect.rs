//! The indirect-call facts of the crate being compiled: a walk over every
//! instance its monomorphic items reach through direct calls, coercions and
//! constants, reading each instance's MIR.
use crate::facts::{Facts, IndirectCall, fn_pointer_key, function_key};
use crate::leaks;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::{
    with_no_trimmed_paths, with_no_visible_paths, with_resolve_crate_name,
};
use rustc_public::mir::alloc::{AllocId, GlobalAlloc};
use rustc_public::mir::mono::{Instance, InstanceKind, StaticDef};
use rustc_public::mir::visit::{Location, MirVisitor};
use rustc_public::mir::{
    Body, CastKind, LocalDecl, PointerCoercion, Rvalue, Terminator, TerminatorKind,
};
use rustc_public::rustc_internal;
use rustc_public::ty::{
    Allocation, ClosureKind, ConstantKind, ExistentialPredicate, ExistentialTraitRef,
    GenericArgKind, MirConst, RigidTy, Ty, TyKind, VtblEntry,
};
use rustc_public::{CrateDef, CrateItem, ItemKind};
use std::collections::BTreeSet;

/// A type as rustc prints it with every definition by its full path: from
/// its crate's name (the local crate's too), not trimmed to a unique name
/// nor shortened to a re-export, so that every crate prints one type alike.
fn canonical_type(tcx: TyCtxt<'_>, ty: Ty) -> String {
    let internal = rustc_internal::internal(tcx, ty);
    with_resolve_crate_name!(with_no_trimmed_paths!(with_no_visible_paths!(
        internal.to_string()
    )))
}

/// A trait's full path, as [`canonical_type`] prints definitions.
fn canonical_trait(tcx: TyCtxt<'_>, principal: &ExistentialTraitRef) -> String {
    let def_id = rustc_internal::internal(tcx, principal.def_id.def_id());
    with_resolve_crate_name!(with_no_trimmed_paths!(with_no_visible_paths!(
        tcx.def_path_str(def_id)
    )))
}

/// Every fact of the crate the compiler analysed.
pub fn crate_facts(tcx: TyCtxt<'_>) -> Facts {
    let mut walk = Walk {
        tcx,
        facts: Facts {
            schema: 2,
            krate: rustc_public::local_crate().name,
            ..Facts::default()
        },
        queue: Vec::new(),
        seen: BTreeSet::new(),
        allocations: BTreeSet::new(),
        unions: Default::default(),
        implementors: Default::default(),
        leaked: Default::default(),
    };
    for item in rustc_public::all_local_items() {
        match item.kind() {
            ItemKind::Static => {
                if let Ok(definition) = StaticDef::try_from(item)
                    && let Ok(initializer) = definition.eval_initializer()
                {
                    let ty = rustc_internal::internal(tcx, definition.ty());
                    walk.allocation(&initializer, Some(ty));
                }
            }
            _ => walk.root(item),
        }
    }
    while let Some(instance) = walk.queue.pop() {
        walk.instance(instance);
    }
    walk.facts
}

struct Walk<'tcx> {
    tcx: TyCtxt<'tcx>,
    facts: Facts,
    queue: Vec<Instance>,
    seen: BTreeSet<String>,
    allocations: BTreeSet<String>,
    /// Types whose unions were already checked.
    unions: std::collections::HashSet<rustc_middle::ty::Ty<'tcx>>,
    /// Types whose contents a trait's `dyn` already records.
    implementors: std::collections::HashSet<(String, rustc_middle::ty::Ty<'tcx>)>,
    /// Types whose contents already leaked.
    leaked: std::collections::HashSet<rustc_middle::ty::Ty<'tcx>>,
}

impl<'tcx> Walk<'tcx> {
    /// The function-pointer types a value of `ty` carries leave their type.
    fn leak(&mut self, ty: rustc_middle::ty::Ty<'tcx>) {
        if !self.leaked.insert(ty) {
            return;
        }
        let contents = leaks::contents(self.tcx, ty);
        self.facts.leaked_types.extend(contents.keys);
        self.facts.leaked_traits.extend(contents.traits);
        self.facts.unknown_leak |= contents.unknown;
    }

    /// A value of `source` becomes one of `target` by a transmute or a
    /// pointer cast: an edge between function-pointer types, nothing when
    /// the two are one type, and otherwise a leak of what `source` carries.
    fn reinterpret(
        &mut self,
        source: rustc_middle::ty::Ty<'tcx>,
        target: rustc_middle::ty::Ty<'tcx>,
    ) {
        let (source_key, target_key) = (leaks::key(source), leaks::key(target));
        if leaks::shape(self.tcx, source) == leaks::shape(self.tcx, target) {
            return;
        }
        if source.is_fn_ptr() && target.is_fn_ptr() {
            self.facts
                .edges
                .entry(target_key)
                .or_default()
                .insert(source_key);
            return;
        }
        // A reinterpreted pointer reaches one memory as both types: what
        // either writes there, the other reads untyped.
        self.leak(source);
        self.leak(target);
    }

    /// The function pointers the unions of `ty` hold leave their type.
    fn unions_of(&mut self, ty: rustc_middle::ty::Ty<'tcx>) {
        if !self.unions.insert(ty) {
            return;
        }
        let contents = leaks::union_contents(self.tcx, ty);
        self.facts.leaked_types.extend(contents.keys);
        self.facts.leaked_traits.extend(contents.traits);
        self.facts.unknown_leak |= contents.unknown;
    }

    fn root(&mut self, item: CrateItem) {
        // Generic items have no instance of their own: their instances come
        // from the calls and coercions that name them.
        if let Ok(instance) = Instance::try_from(item) {
            self.queue.push(instance);
        }
    }

    fn instance(&mut self, instance: Instance) {
        if !self.seen.insert(instance.mangled_name()) {
            return;
        }
        // A shim calling a function pointer as a closure (`<fn() as
        // FnMut<()>>::call_mut`) calls its `Self`; rustc gives it no body.
        if matches!(
            rustc_internal::internal(self.tcx, instance).def,
            rustc_middle::ty::InstanceKind::FnPtrShim(..)
        ) && let Some(GenericArgKind::Type(self_ty)) = instance.args().0.first()
            && is_fn_pointer(*self_ty)
        {
            let key = function_key(&instance.mangled_name());
            let pointer = fn_pointer_key(&canonical_type(self.tcx, *self_ty));
            self.call(&key, IndirectCall::FnPointer(pointer));
            return;
        }
        // A precompiled crate's own functions carry no MIR.
        if !instance.has_body() {
            return;
        }
        let Some(body) = instance.body() else {
            return;
        };

        // A union local reinterprets what its fields hold.
        for local in body.locals() {
            let ty = rustc_internal::internal(self.tcx, local.ty);
            self.unions_of(ty);
        }
        let mut visitor = BodyVisitor {
            walk: self,
            locals: body.locals().to_vec(),
            symbol: function_key(&instance.mangled_name()),
        };
        visitor.visit_body(&body);
    }

    fn call(&mut self, symbol: &str, call: IndirectCall) {
        self.facts
            .calls
            .entry(symbol.to_owned())
            .or_default()
            .insert(call);
    }

    /// `function` becomes a function pointer of type `pointer`.
    fn reified(&mut self, pointer: Ty, function: Instance) {
        self.facts
            .fn_pointers
            .entry(fn_pointer_key(&canonical_type(self.tcx, pointer)))
            .or_default()
            .insert(function_key(&function.mangled_name()));
        self.queue.push(function);
    }

    /// The vtable of `concrete` as a `dyn principal`.
    fn vtable(&mut self, concrete: Ty, principal: &ExistentialTraitRef) {
        let name = canonical_trait(self.tcx, principal);
        // What a `dyn` of the trait can carry, should one leak.
        let internal = rustc_internal::internal(self.tcx, concrete);
        if self.implementors.insert((name.clone(), internal)) {
            let contents = leaks::contents(self.tcx, internal);
            let entry = self.facts.trait_contents.entry(name.clone()).or_default();
            entry.merge(contents);
        }
        let entries = principal.with_self_ty(concrete).vtable_entries();
        let drop = Instance::resolve_drop_in_place(concrete);
        let mut methods = vec![(0, drop)];
        for (index, entry) in entries.into_iter().enumerate() {
            if let VtblEntry::Method(method) = entry {
                methods.push((index, method));
            }
        }
        for (index, method) in methods {
            self.facts
                .vtables
                .entry(name.clone())
                .or_default()
                .entry(index)
                .or_default()
                .insert(function_key(&method.mangled_name()));
            self.queue.push(method);
        }
    }

    /// The functions and vtables a constant's memory of type `ty` points
    /// to. A function in a field of a function-pointer type is a pointer of
    /// that type; one where the type gives none (unknown, an integer, an
    /// erased pointer) leaves its type.
    fn allocation(&mut self, allocation: &Allocation, ty: Option<rustc_middle::ty::Ty<'tcx>>) {
        for (offset, provenance) in &allocation.provenance.ptrs {
            let slot = ty.and_then(|ty| leaks::type_at(self.tcx, ty, *offset as u64));
            self.global(provenance.0, slot);
        }
    }

    fn global(&mut self, id: AllocId, slot: Option<rustc_middle::ty::Ty<'tcx>>) {
        if !self.allocations.insert(format!("{id:?} {slot:?}")) {
            return;
        }
        match GlobalAlloc::from(id) {
            GlobalAlloc::Function(function) => match slot {
                Some(pointer) if pointer.is_fn_ptr() => {
                    self.facts
                        .fn_pointers
                        .entry(leaks::key(pointer))
                        .or_default()
                        .insert(function_key(&function.mangled_name()));
                    self.queue.push(function);
                }
                _ => {
                    if let Some(signature) = function.ty().kind().fn_sig() {
                        let pointer = Ty::from_rigid_kind(RigidTy::FnPtr(signature));
                        self.facts.leaked_functions.insert(
                            function_key(&function.mangled_name()),
                            fn_pointer_key(&canonical_type(self.tcx, pointer)),
                        );
                    }
                    self.queue.push(function);
                }
            },
            GlobalAlloc::VTable(concrete, Some(principal)) => {
                self.vtable(concrete, &principal.skip_binder());
            }
            GlobalAlloc::Memory(memory) => {
                // The memory a pointer field points to has its pointee type.
                let pointee = slot.and_then(|slot| slot.builtin_deref(true));
                self.allocation(&memory, pointee);
            }
            GlobalAlloc::VTable(_, None) | GlobalAlloc::Static(_) | GlobalAlloc::TypeId { .. } => {}
        }
    }
}

struct BodyVisitor<'a, 'tcx> {
    walk: &'a mut Walk<'tcx>,
    locals: Vec<LocalDecl>,
    symbol: String,
}

impl MirVisitor for BodyVisitor<'_, '_> {
    fn visit_terminator(&mut self, terminator: &Terminator, location: Location) {
        // A drop runs the place's drop glue: a `dyn`'s through its vtable's
        // entry 0, any other type's directly.
        if let TerminatorKind::Drop { place, .. } = &terminator.kind
            && let Ok(ty) = place.ty(&self.locals)
        {
            match principal(ty) {
                Some(principal) => self.walk.call(
                    &self.symbol,
                    IndirectCall::Dyn {
                        r#trait: canonical_trait(self.walk.tcx, &principal),
                        entry: 0,
                    },
                ),
                None => {
                    if let TyKind::RigidTy(RigidTy::Dynamic(..)) = ty.kind() {
                        self.walk.call(&self.symbol, IndirectCall::Unknown);
                    } else {
                        self.walk.queue.push(Instance::resolve_drop_in_place(ty));
                    }
                }
            }
        }
        if let TerminatorKind::Call { func, .. } = &terminator.kind
            && let Ok(ty) = func.ty(&self.locals)
        {
            match ty.kind() {
                TyKind::RigidTy(RigidTy::FnDef(def, args)) => {
                    let call = match Instance::resolve(def, &args) {
                        Ok(callee) => match callee.kind {
                            InstanceKind::Virtual { idx } => Some(match dyn_principal(&args) {
                                Some(principal) => IndirectCall::Dyn {
                                    r#trait: canonical_trait(self.walk.tcx, &principal),
                                    entry: idx,
                                },
                                None => IndirectCall::Unknown,
                            }),
                            _ => {
                                self.walk.queue.push(callee);
                                None
                            }
                        },
                        // A call the compiler cannot name here may be any.
                        Err(_) => Some(IndirectCall::Unknown),
                    };
                    if let Some(call) = call {
                        self.walk.call(&self.symbol, call);
                    }
                }
                TyKind::RigidTy(RigidTy::FnPtr(_)) => self.walk.call(
                    &self.symbol,
                    IndirectCall::FnPointer(fn_pointer_key(&canonical_type(self.walk.tcx, ty))),
                ),
                _ => {}
            }
        }
        self.super_terminator(terminator, location);
    }

    fn visit_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        if let Rvalue::Cast(kind, operand, target) = rvalue
            && let Ok(source) = operand.ty(&self.locals)
        {
            match kind {
                CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer(_)) => {
                    if let TyKind::RigidTy(RigidTy::FnDef(def, args)) = source.kind()
                        && let Ok(function) = Instance::resolve_for_fn_ptr(def, &args)
                    {
                        self.walk.reified(*target, function);
                    }
                }
                CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer(_)) => {
                    if let TyKind::RigidTy(RigidTy::Closure(def, args)) = source.kind()
                        && let Ok(function) =
                            Instance::resolve_closure(def, &args, ClosureKind::FnOnce)
                    {
                        self.walk.reified(*target, function);
                    }
                }
                CastKind::PointerCoercion(PointerCoercion::Unsize) => {
                    for (concrete, principal) in unsized_pairs(source, *target) {
                        self.walk.vtable(concrete, &principal);
                    }
                }
                CastKind::FnPtrToPtr | CastKind::PointerExposeAddress => {
                    let source = rustc_internal::internal(self.walk.tcx, source);
                    if source.is_fn_ptr() {
                        self.walk.leak(source);
                    }
                }
                CastKind::Transmute | CastKind::PtrToPtr => {
                    let tcx = self.walk.tcx;
                    let source = rustc_internal::internal(tcx, source);
                    let target = rustc_internal::internal(tcx, *target);
                    self.walk.reinterpret(source, target);
                }
                _ => {}
            }
        }
        self.super_rvalue(rvalue, location);
    }

    fn visit_mir_const(&mut self, constant: &MirConst, location: Location) {
        if let ConstantKind::Allocated(allocation) = constant.kind() {
            let ty = rustc_internal::internal(self.walk.tcx, constant.ty());
            self.walk.allocation(allocation, Some(ty));
        }
        self.super_mir_const(constant, location);
    }
}

fn is_fn_pointer(ty: Ty) -> bool {
    matches!(ty.kind(), TyKind::RigidTy(RigidTy::FnPtr(_)))
}

/// The principal trait of the `dyn` a trait method's `Self` argument is.
fn dyn_principal(args: &rustc_public::ty::GenericArgs) -> Option<ExistentialTraitRef> {
    let GenericArgKind::Type(self_ty) = args.0.first()? else {
        return None;
    };
    principal(*self_ty)
}

fn principal(ty: Ty) -> Option<ExistentialTraitRef> {
    let TyKind::RigidTy(RigidTy::Dynamic(predicates, ..)) = ty.kind() else {
        return None;
    };
    predicates
        .into_iter()
        .find_map(|predicate| match predicate.skip_binder() {
            ExistentialPredicate::Trait(principal) => Some(principal),
            _ => None,
        })
}

/// The concrete types an unsizing coercion from `source` to `target` turns
/// into `dyn` values, with their principal traits: the two types walked in
/// step through references, pointers and generic arguments.
fn unsized_pairs(source: Ty, target: Ty) -> Vec<(Ty, ExistentialTraitRef)> {
    let mut pairs = Vec::new();
    let mut stack = vec![(source, target)];
    while let Some((source, target)) = stack.pop() {
        if let Some(principal) = principal(target) {
            if principal_is_concrete(source) {
                pairs.push((source, principal));
            }
            continue;
        }
        match (source.kind(), target.kind()) {
            (
                TyKind::RigidTy(RigidTy::Ref(_, source, _)),
                TyKind::RigidTy(RigidTy::Ref(_, target, _)),
            )
            | (
                TyKind::RigidTy(RigidTy::RawPtr(source, _)),
                TyKind::RigidTy(RigidTy::RawPtr(target, _)),
            ) => stack.push((source, target)),
            (
                TyKind::RigidTy(RigidTy::Adt(_, source)),
                TyKind::RigidTy(RigidTy::Adt(_, target)),
            ) => {
                for (source, target) in source.0.iter().zip(&target.0) {
                    if let (GenericArgKind::Type(source), GenericArgKind::Type(target)) =
                        (source, target)
                    {
                        stack.push((*source, *target));
                    }
                }
            }
            _ => {}
        }
    }
    pairs
}

/// Whether a coerced type is a concrete one, not a `dyn` upcast.
fn principal_is_concrete(ty: Ty) -> bool {
    principal(ty).is_none()
}

#[allow(dead_code, reason = "the body type the visitor walks")]
type Walked = Body;
