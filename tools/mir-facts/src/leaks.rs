//! The function-pointer types a value of a type can carry: through its
//! references and pointers (reinterpreting a reference reinterprets what it
//! points to), its fields, a closure's captures and a coroutine's saved
//! locals. A `dyn` part carries its principal trait: its vtables and what
//! every type made such a `dyn` carries. A part whose contents cannot be
//! enumerated (a `dyn` without a principal trait, an opaque or generic type)
//! makes the contents unknown.
use crate::facts::fn_pointer_key;
use rustc_middle::ty::Unnormalized;
use rustc_middle::ty::print::{
    with_no_trimmed_paths, with_no_visible_paths, with_resolve_crate_name,
};
use rustc_middle::ty::{self, EarlyBinder, Ty, TyCtxt, TypingEnv};
use std::collections::{BTreeSet, HashSet};

/// What a type can carry.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Contents {
    /// The keys of the function-pointer types it can carry.
    pub keys: BTreeSet<String>,
    /// The principal traits of the `dyn` values it can carry.
    pub traits: BTreeSet<String>,
    /// A part whose contents are unknown.
    pub unknown: bool,
}

impl Contents {
    pub fn merge(&mut self, other: Contents) {
        self.keys.extend(other.keys);
        self.traits.extend(other.traits);
        self.unknown |= other.unknown;
    }
}

/// A trait's full path, as the facts key traits.
pub fn trait_key(tcx: TyCtxt<'_>, def_id: rustc_span::def_id::DefId) -> String {
    with_resolve_crate_name!(with_no_trimmed_paths!(with_no_visible_paths!(
        tcx.def_path_str(def_id)
    )))
}

/// The key of an internal function-pointer type, as the facts key them.
pub fn key(ty: Ty<'_>) -> String {
    fn_pointer_key(&with_resolve_crate_name!(with_no_trimmed_paths!(
        with_no_visible_paths!(ty.to_string())
    )))
}

/// The function-pointer types `ty` can carry.
pub fn contents<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> Contents {
    let mut contents = Contents::default();
    let mut seen = HashSet::new();
    let mut stack = vec![ty];
    while let Some(ty) = stack.pop() {
        // A monomorphic type graph is finite: each type is visited once.
        if !seen.insert(ty) {
            continue;
        }
        let typing = TypingEnv::fully_monomorphized();
        let ty = tcx.normalize_erasing_regions(typing, Unnormalized::new_wip(ty));
        match ty.kind() {
            ty::FnPtr(..) => {
                contents.keys.insert(key(ty));
            }
            ty::Bool
            | ty::Char
            | ty::Int(_)
            | ty::Uint(_)
            | ty::Float(_)
            | ty::Str
            | ty::Never
            | ty::FnDef(..)
            | ty::Foreign(_) => {}
            ty::Ref(_, pointee, _)
            | ty::RawPtr(pointee, _)
            | ty::Slice(pointee)
            | ty::Array(pointee, _) => {
                stack.push(*pointee);
            }
            ty::Pat(inner, _) => stack.push(*inner),
            ty::Tuple(fields) => stack.extend(fields.iter()),
            ty::Adt(def, args) => {
                for field in def.all_fields() {
                    stack.push(field.ty(tcx, args).skip_norm_wip());
                }
            }
            ty::Closure(_, args) => stack.extend(args.as_closure().upvar_tys().iter()),
            ty::CoroutineClosure(_, args) => {
                stack.extend(args.as_coroutine_closure().upvar_tys().iter())
            }
            ty::Coroutine(def_id, args) => {
                stack.extend(args.as_coroutine().upvar_tys().iter());
                match tcx.coroutine_layout(*def_id, args) {
                    Ok(layout) => {
                        for saved in layout.field_tys.iter() {
                            stack.push(
                                EarlyBinder::bind(saved.ty)
                                    .instantiate(tcx, args)
                                    .skip_norm_wip(),
                            );
                        }
                    }
                    Err(_) => contents.unknown = true,
                }
            }
            ty::Dynamic(predicates, ..) => match predicates.principal_def_id() {
                Some(principal) => {
                    contents.traits.insert(trait_key(tcx, principal));
                }
                None => contents.unknown = true,
            },
            ty::UnsafeBinder(_)
            | ty::CoroutineWitness(..)
            | ty::Alias(..)
            | ty::Param(_)
            | ty::Bound(..)
            | ty::Placeholder(_)
            | ty::Infer(_)
            | ty::Error(_) => contents.unknown = true,
        }
    }
    contents
}

/// The shape of a view of memory: `ty` with every `#[repr(transparent)]`
/// wrapper replaced by its one field of non-zero size (which has its layout
/// by definition: `UnsafeCell`, `MaybeUninit`, `ManuallyDrop`, `NonNull`,
/// `Pin`), every reference and raw pointer as one pointer kind, and a
/// pointer to a slice or an array as a pointer to its element. Two types of
/// one shape view the same memory alike: reinterpreting one as the other
/// keeps every function pointer in its type.
pub fn shape<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> String {
    key(strip(tcx, ty, 0))
}

fn strip<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, depth: u32) -> Ty<'tcx> {
    let typing = TypingEnv::fully_monomorphized();
    let ty = tcx.normalize_erasing_regions(typing, Unnormalized::new_wip(ty));
    if depth > 32 {
        return ty;
    }
    match ty.kind() {
        ty::Adt(def, args) if def.repr().transparent() => {
            let inner = def
                .all_fields()
                .map(|field| field.ty(tcx, args).skip_norm_wip())
                .find(|field| {
                    tcx.layout_of(typing.as_query_input(*field))
                        .is_ok_and(|layout| !layout.is_zst())
                });
            match inner {
                Some(inner) => strip(tcx, inner, depth + 1),
                None => ty,
            }
        }
        ty::Ref(_, pointee, _) | ty::RawPtr(pointee, _) => {
            let mut pointee = *pointee;
            while let ty::Slice(element) | ty::Array(element, _) = pointee.kind() {
                pointee = *element;
            }
            Ty::new_imm_ptr(tcx, strip(tcx, pointee, depth + 1))
        }
        // A pattern type (`NonNull`'s non-null pointer) has its base's layout.
        ty::Pat(inner, _) => strip(tcx, *inner, depth + 1),
        _ => ty,
    }
}

/// Whether `ty` is a thin pointer (or a transparent wrapper of one).
pub fn is_thin_pointer<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    let typing = TypingEnv::fully_monomorphized();
    let stripped = strip(tcx, ty, 0);
    match stripped.kind() {
        ty::RawPtr(pointee, _) => pointee.is_sized(tcx, typing),
        _ => false,
    }
}

/// The function-pointer types the unions within `ty` hold where a union
/// reinterprets one field as another: fields of non-zero size of more than
/// one shape. A union of one such field (`MaybeUninit`) reinterprets
/// nothing.
pub fn union_contents<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> Contents {
    let mut found = Contents::default();
    let mut seen = HashSet::new();
    let mut stack = vec![ty];
    while let Some(ty) = stack.pop() {
        if !seen.insert(ty) {
            continue;
        }
        let typing = TypingEnv::fully_monomorphized();
        let ty = tcx.normalize_erasing_regions(typing, Unnormalized::new_wip(ty));
        match ty.kind() {
            ty::Adt(def, args) => {
                if def.is_union() {
                    let fields: Vec<Ty<'tcx>> = def
                        .all_fields()
                        .map(|field| field.ty(tcx, args).skip_norm_wip())
                        .filter(|field| {
                            tcx.layout_of(typing.as_query_input(*field))
                                .is_ok_and(|layout| !layout.is_zst())
                        })
                        .collect();
                    let shapes: BTreeSet<String> =
                        fields.iter().map(|field| shape(tcx, *field)).collect();
                    if shapes.len() > 1 {
                        found.merge(contents(tcx, ty));
                    }
                    // Each field may hold unions of its own.
                    stack.extend(fields);
                } else {
                    for field in def.all_fields() {
                        stack.push(field.ty(tcx, args).skip_norm_wip());
                    }
                }
            }
            ty::Tuple(fields) => stack.extend(fields.iter()),
            ty::Array(inner, _) | ty::Slice(inner) | ty::Pat(inner, _) => stack.push(*inner),
            ty::Closure(_, args) => stack.extend(args.as_closure().upvar_tys().iter()),
            ty::Coroutine(def_id, args) => {
                stack.extend(args.as_coroutine().upvar_tys().iter());
                if let Ok(layout) = tcx.coroutine_layout(*def_id, args) {
                    for saved in layout.field_tys.iter() {
                        stack.push(
                            EarlyBinder::bind(saved.ty)
                                .instantiate(tcx, args)
                                .skip_norm_wip(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    found
}

/// The type of the field of `ty` that holds byte `offset`, through structs,
/// tuples, arrays, closures and every variant of an enum (when the variants
/// agree); `None` where the layout gives no single field.
pub fn type_at<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, offset: u64) -> Option<Ty<'tcx>> {
    use rustc_middle::ty::layout::{LayoutCx, TyAndLayout};
    let typing = TypingEnv::fully_monomorphized();
    let cx = LayoutCx::new(tcx, typing);
    let mut ty = tcx.normalize_erasing_regions(typing, Unnormalized::new_wip(ty));
    let mut offset = offset;
    for _ in 0..64 {
        if offset == 0 && (ty.is_fn_ptr() || ty.is_ref() || ty.is_raw_ptr() || ty.is_box()) {
            return Some(ty);
        }
        let layout: TyAndLayout<'tcx> = tcx.layout_of(typing.as_query_input(ty)).ok()?;
        let fields: Vec<(u64, Ty<'tcx>)> = match ty.kind() {
            ty::Adt(def, args) if def.is_struct() => def
                .non_enum_variant()
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    (
                        layout.fields.offset(index).bytes(),
                        field.ty(tcx, args).skip_norm_wip(),
                    )
                })
                .collect(),
            ty::Adt(def, args) if def.is_enum() => {
                let mut candidates = Vec::new();
                for (variant_index, variant) in def.variants().iter_enumerated() {
                    let variant_layout = layout.for_variant(&cx, variant_index);
                    for (index, field) in variant.fields.iter().enumerate() {
                        candidates.push((
                            variant_layout.fields.offset(index).bytes(),
                            field.ty(tcx, args).skip_norm_wip(),
                        ));
                    }
                }
                candidates
            }
            ty::Tuple(elements) => elements
                .iter()
                .enumerate()
                .map(|(index, element)| (layout.fields.offset(index).bytes(), element))
                .collect(),
            ty::Closure(_, args) => args
                .as_closure()
                .upvar_tys()
                .iter()
                .enumerate()
                .map(|(index, upvar)| (layout.fields.offset(index).bytes(), upvar))
                .collect(),
            ty::Array(element, _) => {
                let size = tcx
                    .layout_of(typing.as_query_input(*element))
                    .ok()?
                    .size
                    .bytes();
                if size == 0 {
                    return None;
                }
                offset %= size;
                ty = tcx.normalize_erasing_regions(typing, Unnormalized::new_wip(*element));
                continue;
            }
            _ => return None,
        };
        // The field (of any variant) whose bytes hold the offset; variants
        // that disagree on its type give no single type.
        let mut found: Option<(u64, Ty<'tcx>)> = None;
        for (start, field) in fields {
            let field = tcx.normalize_erasing_regions(typing, Unnormalized::new_wip(field));
            let size = tcx
                .layout_of(typing.as_query_input(field))
                .ok()?
                .size
                .bytes();
            if offset >= start && offset < start + size.max(1) {
                match found {
                    None => found = Some((start, field)),
                    Some((_, other)) if other == field => {}
                    Some(_) => return None,
                }
            }
        }
        let (start, field) = found?;
        offset -= start;
        ty = field;
    }
    None
}
