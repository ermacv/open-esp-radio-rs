//! The indirect-call facts of the crate being compiled: a walk over every
//! instance its monomorphic items reach through direct calls, coercions and
//! constants, reading each instance's MIR.
use crate::facts::{Facts, IndirectCall, fn_pointer_key};
use rustc_public::mir::alloc::{AllocId, GlobalAlloc};
use rustc_public::mir::mono::{Instance, InstanceKind, StaticDef};
use rustc_public::mir::visit::{Location, MirVisitor};
use rustc_public::mir::{
    Body, CastKind, LocalDecl, PointerCoercion, Rvalue, Terminator, TerminatorKind,
};
use rustc_public::ty::{
    Allocation, ClosureKind, ConstantKind, ExistentialPredicate, ExistentialTraitRef,
    GenericArgKind, MirConst, RigidTy, Ty, TyKind, VtblEntry,
};
use rustc_public::{CrateDef, CrateItem, ItemKind};
use std::collections::BTreeSet;

/// Every fact of the crate the compiler analysed.
pub fn crate_facts() -> Facts {
    let mut walk = Walk {
        facts: Facts {
            schema: 1,
            krate: rustc_public::local_crate().name,
            ..Facts::default()
        },
        queue: Vec::new(),
        seen: BTreeSet::new(),
        allocations: BTreeSet::new(),
    };
    for item in rustc_public::all_local_items() {
        match item.kind() {
            ItemKind::Static => {
                if let Ok(definition) = StaticDef::try_from(item)
                    && let Ok(initializer) = definition.eval_initializer()
                {
                    walk.allocation(&initializer);
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

struct Walk {
    facts: Facts,
    queue: Vec<Instance>,
    seen: BTreeSet<String>,
    allocations: BTreeSet<String>,
}

impl Walk {
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
        // A precompiled crate's own functions carry no MIR.
        if !instance.has_body() {
            return;
        }
        let Some(body) = instance.body() else {
            return;
        };
        let mut visitor = BodyVisitor {
            walk: self,
            locals: body.locals().to_vec(),
            symbol: instance.mangled_name(),
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
            .entry(fn_pointer_key(&pointer.to_string()))
            .or_default()
            .insert(function.mangled_name());
        self.queue.push(function);
    }

    /// The vtable of `concrete` as a `dyn principal`.
    fn vtable(&mut self, concrete: Ty, principal: &ExistentialTraitRef) {
        let name = principal.def_id.name();
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
                .insert(method.mangled_name());
            self.queue.push(method);
        }
    }

    /// The functions and vtables a constant's memory points to.
    fn allocation(&mut self, allocation: &Allocation) {
        for (_, provenance) in &allocation.provenance.ptrs {
            self.global(provenance.0);
        }
    }

    fn global(&mut self, id: AllocId) {
        if !self.allocations.insert(format!("{id:?}")) {
            return;
        }
        match GlobalAlloc::from(id) {
            GlobalAlloc::Function(function) => {
                // A function in a constant is a pointer of its own signature.
                if let Some(signature) = function.ty().kind().fn_sig() {
                    let pointer = Ty::from_rigid_kind(RigidTy::FnPtr(signature));
                    self.reified(pointer, function);
                }
            }
            GlobalAlloc::VTable(concrete, Some(principal)) => {
                self.vtable(concrete, &principal.skip_binder());
            }
            GlobalAlloc::Memory(memory) => self.allocation(&memory),
            GlobalAlloc::VTable(_, None) | GlobalAlloc::Static(_) | GlobalAlloc::TypeId { .. } => {}
        }
    }
}

struct BodyVisitor<'a> {
    walk: &'a mut Walk,
    locals: Vec<LocalDecl>,
    symbol: String,
}

impl MirVisitor for BodyVisitor<'_> {
    fn visit_terminator(&mut self, terminator: &Terminator, location: Location) {
        if let TerminatorKind::Call { func, .. } = &terminator.kind
            && let Ok(ty) = func.ty(&self.locals)
        {
            match ty.kind() {
                TyKind::RigidTy(RigidTy::FnDef(def, args)) => {
                    let call = match Instance::resolve(def, &args) {
                        Ok(callee) => match callee.kind {
                            InstanceKind::Virtual { idx } => Some(match dyn_principal(&args) {
                                Some(principal) => IndirectCall::Dyn {
                                    r#trait: principal.def_id.name(),
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
                    IndirectCall::FnPointer(fn_pointer_key(&ty.to_string())),
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
                CastKind::Transmute if is_fn_pointer(*target) && source != *target => {
                    self.walk
                        .facts
                        .polluted
                        .insert(fn_pointer_key(&target.to_string()));
                }
                _ => {}
            }
        }
        self.super_rvalue(rvalue, location);
    }

    fn visit_mir_const(&mut self, constant: &MirConst, location: Location) {
        if let ConstantKind::Allocated(allocation) = constant.kind() {
            self.walk.allocation(allocation);
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
