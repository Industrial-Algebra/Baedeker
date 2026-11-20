// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Mutable runtime state for an instantiated module: linear memories and
//! globals. See [Spec §4.4](https://webassembly.github.io/spec/core/exec/runtime.html).

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lower::{RegConstInstr, RegDataMode, RegElemValue, RegElementMode, RegModule};
use crate::runtime::gcheap::{GcHeap, GcSnapshot, GcStats, GcTypeId, GcTypeKind, HeapId};
use crate::runtime::gpu::{GpuBackend, GpuError, GpuKernelId};
use crate::runtime::host::HostFunction;
use crate::runtime::{
    RuntimeError, RuntimeErrorKind, RuntimeTrap, Value, default_for_storage, trap,
};
use crate::types::{
    ClosedDefinedType, CompType, FieldType, FuncIdx, FuncType, GlobalType, HeapType, Limits,
    MemType, RefType, StorageType, SubType, TableType, TypeIdx, ValType,
};

use crate::runtime::verify::F32_ADD_WGSL;

/// Default element count at or above which bulk SIMD work dispatches to
/// GPU. Below this, CPU execution avoids dispatch overhead. Per-platform
/// tuning (unified-memory vs discrete GPU) adjusts this at runtime.
pub const DEFAULT_OFFLOAD_THRESHOLD: usize = 1024;

/// Size of one WebAssembly memory page in bytes.
pub const PAGE_SIZE: usize = 65536;

/// A provider module's full, kind-preserving type section, registered on
/// [`Imports`] so that concrete reference types in its exports can be
/// compared structurally across modules instead of by raw type index.
#[derive(Debug, Clone, Default)]
struct TypeContext {
    types: Vec<SubType>,
    rec_groups: Vec<(u32, u32)>,
}

/// Entities provided by the host to satisfy a module's state imports
/// (memories, globals, tables), resolved eagerly at instantiation.
/// Function imports resolve separately and lazily via
/// [`Store::register_host_func`].
#[derive(Debug, Default)]
pub struct Imports {
    memories: Vec<MemoryProvider>,
    globals: Vec<GlobalProvider>,
    tables: Vec<TableProvider>,
    tags: Vec<TagProvider>,
    /// Per-provider-module type sections, keyed by the import module name.
    type_contexts: alloc::collections::BTreeMap<String, TypeContext>,
    /// GC heaps of provider stores. Registered so that a `GcRef` value
    /// crossing a shared global/table (or a cross-instance call/host round
    /// trip) resolves against the heap that created it instead of being
    /// misread as a local object id.
    provider_heaps: Vec<alloc::rc::Rc<core::cell::RefCell<GcHeap>>>,
}

impl Imports {
    /// An empty import set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the defining module's full type section under `module`, so
    /// that concrete reference types exported by that module are matched
    /// structurally (rolled closed forms) at link time rather than by raw
    /// type index. Call once per provider module before the fluent
    /// memory/global/table/tag registration.
    ///
    /// Precondition: invoke this **exactly once per `module` name**. The
    /// registration is last-wins — a second call for the same name replaces
    /// the earlier type section, so any provider registered against the
    /// first context would be reinterpreted against the second. There is no
    /// merge or duplicate detection here.
    pub fn with_module_types(mut self, module: &str, provider: &RegModule) -> Self {
        self.type_contexts.insert(
            module.into(),
            TypeContext {
                types: provider.type_defs.clone(),
                rec_groups: provider.rec_groups.clone(),
            },
        );
        self
    }

    /// Register a provider store's GC heap.
    ///
    /// Call once per store whose GC values (shared globals/tables, thrown
    /// exception payloads, or cross-instance call arguments) may reach this
    /// consumer. Registration is idempotent and by heap identity, so
    /// registering an unrelated heap is harmless; the consumer only resolves
    /// a `GcRef` whose [`HeapId`] matches a registered heap.
    pub fn provider_heap(mut self, heap: alloc::rc::Rc<core::cell::RefCell<GcHeap>>) -> Self {
        self.provider_heaps.push(heap);
        self
    }

    /// Provide an imported memory `(module, name)` (fresh allocation).
    pub fn memory(mut self, module: &str, name: &str, ty: MemType) -> Self {
        self.memories.push(MemoryProvider {
            module: module.into(),
            name: name.into(),
            ty,
            shared: None,
        });
        self
    }

    /// Provide an imported memory `(module, name)` linked from another
    /// module's exports (shared handle).
    pub fn shared_memory(
        mut self,
        module: &str,
        name: &str,
        ty: MemType,
        shared: alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>,
    ) -> Self {
        self.memories.push(MemoryProvider {
            module: module.into(),
            name: name.into(),
            ty,
            shared: Some(shared),
        });
        self
    }

    /// Provide an imported global `(module, name)` with its value.
    pub fn global(mut self, module: &str, name: &str, ty: GlobalType, value: Value) -> Self {
        self.globals.push(GlobalProvider {
            module: module.into(),
            name: name.into(),
            ty,
            value: Some(value),
            shared: None,
        });
        self
    }

    /// Provide an imported global `(module, name)` linked from another
    /// module's exports (shared cell).
    pub fn shared_global(
        mut self,
        module: &str,
        name: &str,
        ty: GlobalType,
        shared: alloc::rc::Rc<core::cell::Cell<Value>>,
    ) -> Self {
        self.globals.push(GlobalProvider {
            module: module.into(),
            name: name.into(),
            ty,
            value: None,
            shared: Some(shared),
        });
        self
    }

    /// Provide an imported table `(module, name)` (fresh allocation).
    pub fn table(mut self, module: &str, name: &str, ty: TableType) -> Self {
        self.tables.push(TableProvider {
            module: module.into(),
            name: name.into(),
            ty,
            shared: None,
        });
        self
    }

    /// Provide an imported table `(module, name)` linked from another
    /// module's exports (shared handle).
    pub fn shared_table(
        mut self,
        module: &str,
        name: &str,
        ty: TableType,
        shared: alloc::rc::Rc<core::cell::RefCell<crate::runtime::Table>>,
    ) -> Self {
        self.tables.push(TableProvider {
            module: module.into(),
            name: name.into(),
            ty,
            shared: Some(shared),
        });
        self
    }

    /// Provide an imported tag `(module, name)` bound to the defining
    /// instance's tag identity (from that instance's `tag_binding`).
    #[allow(clippy::too_many_arguments)]
    pub fn tag(
        mut self,
        module: &str,
        name: &str,
        identity: (u32, u32),
        ty: FuncType,
        expanded_params: Option<Vec<ExpandedVal>>,
        rec: RecProvenance,
        closed: Option<ClosedDefinedType>,
    ) -> Self {
        self.tags.push(TagProvider {
            module: module.into(),
            name: name.into(),
            identity,
            ty,
            expanded_params,
            rec,
            closed,
        });
        self
    }
}

/// A host-provided memory for import resolution.
#[derive(Debug)]
struct MemoryProvider {
    module: String,
    name: String,
    ty: MemType,
    /// Shared handle when the memory is linked from another module.
    shared: Option<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>>,
}

/// A host-provided global for import resolution.
#[derive(Debug)]
struct GlobalProvider {
    module: String,
    name: String,
    ty: GlobalType,
    value: Option<Value>,
    /// Shared cell when the global is linked from another module.
    shared: Option<alloc::rc::Rc<core::cell::Cell<Value>>>,
}

/// A host-provided table for import resolution.
#[derive(Debug)]
struct TableProvider {
    module: String,
    name: String,
    ty: TableType,
    /// Shared handle when the table is linked from another module.
    shared: Option<alloc::rc::Rc<core::cell::RefCell<crate::runtime::Table>>>,
}

/// A host-provided tag for import resolution.
#[derive(Debug)]
struct TagProvider {
    module: String,
    name: String,
    identity: (u32, u32),
    ty: FuncType,
    expanded_params: Option<Vec<ExpandedVal>>,
    rec: RecProvenance,
    /// The tag's OWN referenced function type's complete closed form
    /// (finality + supertypes + comptype), computed by the defining module.
    /// Compared at link time so an explicit non-final `(sub (func))` tag
    /// type does not conflate with a synthesized plain final `(func)`.
    closed: Option<ClosedDefinedType>,
}

/// The null value for a table's element type.
fn table_null(ty: TableType) -> Value {
    match ty.elem {
        crate::types::RefType::ExternRef
        | crate::types::RefType::Typed {
            heap: crate::types::HeapType::Extern,
            ..
        } => Value::ExternRef(None),
        crate::types::RefType::Typed {
            heap: crate::types::HeapType::Exn,
            ..
        } => Value::ExnRef(None),
        _ => Value::FuncRef(None),
    }
}

/// Allocate `n` copies of `value` with a fallible reservation, failing
/// cleanly instead of aborting on huge sizes.
fn try_alloc_n<T: Clone>(n: usize, value: T) -> Option<Vec<T>> {
    let mut vec = Vec::new();
    vec.try_reserve(n).ok()?;
    vec.resize(n, value);
    Some(vec)
}

/// Resolve every defined GC aggregate type in `module` to a heap-interned
/// `GcTypeId` once, before any instance state exists.
///
/// The execution path then looks metadata up by module-local type index and
/// performs zero layout/closed-form allocation per allocation. The layout is
/// cloned fallibly (the only per-type allocation here) and interned by its
/// cross-module closed form, so structurally identical aggregates at
/// different local indices share an id. A refusal (OOM) is a clean
/// `ResourceLimitExceeded` with no partial instance.
fn precompute_gc_type_ids(
    module: &RegModule,
    heap: &mut GcHeap,
) -> Result<Vec<Option<GcTypeId>>, RuntimeError> {
    let mut out: Vec<Option<GcTypeId>> = Vec::new();
    out.try_reserve(module.type_defs.len())
        .map_err(|_| allocation_failed("gc type cache"))?;
    for idx in 0..module.type_defs.len() as u32 {
        let type_idx = TypeIdx(idx);
        let entry = match module.type_defs.get(idx as usize).map(|sub| &sub.comp) {
            Some(CompType::Struct(fields)) => {
                let mut cloned = Vec::new();
                cloned
                    .try_reserve(fields.len())
                    .map_err(|_| allocation_failed("gc type layout"))?;
                cloned.extend_from_slice(fields);
                let kind = GcTypeKind::Struct(cloned);
                let closed = ClosedDefinedType::of(&module.type_defs, &module.rec_groups, type_idx)
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownType { type_idx: idx },
                    })?;
                let supers = gc_direct_super_ids(module, idx, &out)?;
                Some(heap.intern_closed_supers(closed, kind, &supers)?)
            }
            Some(CompType::Array(field)) => {
                let kind = GcTypeKind::Array(*field);
                let closed = ClosedDefinedType::of(&module.type_defs, &module.rec_groups, type_idx)
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownType { type_idx: idx },
                    })?;
                let supers = gc_direct_super_ids(module, idx, &out)?;
                Some(heap.intern_closed_supers(closed, kind, &supers)?)
            }
            Some(CompType::Func(_)) | None => None,
        };
        out.push(entry);
    }
    Ok(out)
}

/// The declared type index of every function in the module's index space,
/// imported functions first. `RegModule.funcs` holds DEFINED functions only;
/// their `idx` is the index-space position, so imported slots are filled from
/// the import declarations and defined slots from `funcs`.
fn func_type_idxs_of(module: &RegModule) -> Vec<crate::types::TypeIdx> {
    let mut out: Vec<crate::types::TypeIdx> = module
        .imported_funcs
        .iter()
        .map(|import| import.type_idx)
        .collect();
    let total = module
        .funcs
        .iter()
        .map(|func| func.idx.0 as usize + 1)
        .max()
        .unwrap_or(0)
        .max(out.len());
    out.resize(total, crate::types::TypeIdx(0));
    for func in &module.funcs {
        if let Some(slot) = out.get_mut(func.idx.0 as usize) {
            *slot = func.type_idx;
        }
    }
    out
}

/// The already-interned heap ids of `idx`'s declared supertypes. Precompute
/// visits the type section in index order and validation guarantees every
/// supertype has a smaller index, so each supertype is present in `interned`
/// (the "reachable only as supertypes" case is impossible: supertypes are
/// themselves type-section entries).
fn gc_direct_super_ids(
    module: &RegModule,
    idx: u32,
    interned: &[Option<GcTypeId>],
) -> Result<Vec<GcTypeId>, RuntimeError> {
    let mut out: Vec<GcTypeId> = Vec::new();
    if let Some(sub) = module.type_defs.get(idx as usize) {
        out.try_reserve(sub.supertypes.len())
            .map_err(|_| allocation_failed("gc type ancestry"))?;
        for &super_idx in &sub.supertypes {
            if let Some(Some(id)) = interned.get(super_idx as usize) {
                out.push(*id);
            }
        }
    }
    Ok(out)
}

/// Snapshot the unique ids of provider heaps, dropping duplicates so a
/// heap registered twice resolves identically.
fn register_provider_heaps(
    heaps: &[alloc::rc::Rc<core::cell::RefCell<GcHeap>>],
) -> Vec<(HeapId, alloc::rc::Rc<core::cell::RefCell<GcHeap>>)> {
    let mut out: Vec<(HeapId, alloc::rc::Rc<core::cell::RefCell<GcHeap>>)> = Vec::new();
    for heap in heaps {
        let id = heap.borrow().heap_id();
        if !out.iter().any(|(known, _)| *known == id) {
            out.push((id, heap.clone()));
        }
    }
    out
}

/// Spec limits matching: provided limits must be at least as permissive as
/// declared import limits.
fn limits_match(provided: &Limits, declared: &Limits) -> bool {
    // Index type is part of the entity type: an i32 and an i64 memory
    // (or table) are different types and never link.
    if provided.is64 != declared.is64 {
        return false;
    }
    // Threads pin: shared is part of the memory type — a shared memory
    // never links against an unshared one, in either direction.
    if provided.shared != declared.shared {
        return false;
    }
    if provided.min < declared.min {
        return false;
    }
    match (provided.max, declared.max) {
        (_, None) => true,
        (Some(provided), Some(declared)) => provided <= declared,
        (None, Some(_)) => false,
    }
}

/// Make GC-heap knowledge symmetric across a link group: every member,
/// including the one just registered, learns every peer's heap. Peers that
/// are currently borrowed (e.g. executing) are skipped; their next
/// registration pass catches up.
fn register_group_heaps_symmetric(group: &alloc::rc::Rc<core::cell::RefCell<LinkGroup>>) {
    let heaps: Vec<alloc::rc::Rc<core::cell::RefCell<GcHeap>>> = group
        .borrow()
        .iter()
        .filter_map(|(_, (_, store))| store.try_borrow().ok().map(|s| s.gc_heap.clone()))
        .collect();
    let members: Vec<alloc::rc::Rc<core::cell::RefCell<Store>>> = group
        .borrow()
        .iter()
        .map(|(_, (_, store))| store.clone())
        .collect();
    for member in members {
        let Ok(mut store) = member.try_borrow_mut() else {
            continue;
        };
        let own_id = store.gc_heap.borrow().heap_id();
        for heap in &heaps {
            let id = heap.borrow().heap_id();
            if id != own_id && !store.foreign_heaps.iter().any(|(known, _)| *known == id) {
                store.foreign_heaps.push((id, heap.clone()));
            }
        }
    }
}

fn allocation_failed(what: &'static str) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::ResourceLimitExceeded { what },
    }
}

fn unknown_import(module: &str, name: &str) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::UnknownImport {
            module: module.into(),
            name: name.into(),
        },
    }
}

fn import_type_mismatch(module: &str, name: &str) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::ImportTypeMismatch {
            module: module.into(),
            name: name.into(),
        },
    }
}

/// Whether a provided global type matches a declared import type per the
/// spec's import matching: immutable globals are covariant (provided may be
/// a subtype of declared), mutable globals are invariant (types must be
/// exactly equal), and mutability must match.
fn global_type_matches(provided: &GlobalType, declared: &GlobalType) -> bool {
    use crate::types::{HeapType, Mutability, RefType, ValType};

    fn valtype_is_subtype(provided: &ValType, declared: &ValType) -> bool {
        match (provided, declared) {
            (ValType::Ref(provided), ValType::Ref(declared)) => match (provided, declared) {
                (RefType::FuncRef, RefType::FuncRef) | (RefType::ExternRef, RefType::ExternRef) => {
                    true
                }
                (
                    RefType::Typed {
                        nullable: provided_nullable,
                        heap: provided_heap,
                    },
                    RefType::Typed {
                        nullable: declared_nullable,
                        heap: declared_heap,
                    },
                ) => {
                    (!*provided_nullable || *declared_nullable)
                        && provided_heap.is_subtype_of(*declared_heap)
                }
                (RefType::Typed { heap, .. }, RefType::FuncRef) => {
                    heap.is_subtype_of(HeapType::Func)
                }
                (RefType::Typed { heap, .. }, RefType::ExternRef) => *heap == HeapType::Extern,
                _ => false,
            },
            (provided, declared) => provided == declared,
        }
    }

    match (provided.mutability, declared.mutability) {
        (Mutability::Const, Mutability::Const) => {
            valtype_is_subtype(&provided.val_type, &declared.val_type)
        }
        (Mutability::Var, Mutability::Var) => provided.val_type == declared.val_type,
        _ => false,
    }
}

/// Module-aware heap matching: concrete-vs-concrete via structural
/// closed-form identity or a declared-supertype chain across the two type
/// sections, concrete-vs-abstract via the kind-aware lattice,
/// abstract-vs-abstract via the context-free lattice. Delegates to the
/// shared [`crate::types::defined_type_matches`] relation so the runner's
/// link checks and the store agree on one rule.
fn heaptype_matches_ctx(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: HeapType,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: HeapType,
) -> bool {
    match (provided, declared) {
        (HeapType::Type(a), HeapType::Type(b)) => crate::types::defined_type_matches(
            provided_types,
            provided_recs,
            a,
            declared_types,
            declared_recs,
            b,
        ),
        (HeapType::Type(a), expected) => match crate::types::concrete_kind(provided_types, a) {
            Some(crate::types::ConcreteKind::Struct) => {
                matches!(expected, HeapType::Any | HeapType::Eq | HeapType::Struct)
            }
            Some(crate::types::ConcreteKind::Array) => {
                matches!(expected, HeapType::Any | HeapType::Eq | HeapType::Array)
            }
            Some(crate::types::ConcreteKind::Func) => matches!(expected, HeapType::Func),
            None => false,
        },
        (found, expected) => found.is_subtype_of(expected),
    }
}

fn reftype_matches_ctx(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: RefType,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: RefType,
) -> bool {
    (!provided.is_nullable() || declared.is_nullable())
        && heaptype_matches_ctx(
            provided_types,
            provided_recs,
            provided.heap_type(),
            declared_types,
            declared_recs,
            declared.heap_type(),
        )
}

fn valtype_matches_ctx(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: &ValType,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: &ValType,
) -> bool {
    match (provided, declared) {
        (ValType::Num(a), ValType::Num(b)) => a == b,
        (ValType::Vec(a), ValType::Vec(b)) => a == b,
        (ValType::Ref(a), ValType::Ref(b)) => reftype_matches_ctx(
            provided_types,
            provided_recs,
            *a,
            declared_types,
            declared_recs,
            *b,
        ),
        _ => false,
    }
}

/// Exact structural identity of two reference types across modules — used
/// for table element types, which are matched invariantly.
fn reftype_identical_ctx(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: RefType,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: RefType,
) -> bool {
    if provided.is_nullable() != declared.is_nullable() {
        return false;
    }
    match (provided.heap_type(), declared.heap_type()) {
        (HeapType::Type(a), HeapType::Type(b)) => crate::types::defined_types_equivalent(
            provided_types,
            provided_recs,
            a,
            declared_types,
            declared_recs,
            b,
        ),
        (a, b) => a == b,
    }
}

fn valtype_identical_ctx(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: &ValType,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: &ValType,
) -> bool {
    match (provided, declared) {
        (ValType::Ref(a), ValType::Ref(b)) => reftype_identical_ctx(
            provided_types,
            provided_recs,
            *a,
            declared_types,
            declared_recs,
            *b,
        ),
        (a, b) => a == b,
    }
}

/// Module-aware global type matching: immutable globals are covariant,
/// mutable globals invariant, with concrete reference types resolved
/// structurally against each side's defining type section.
fn global_type_matches_ctx(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: &GlobalType,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: &GlobalType,
) -> bool {
    match (provided.mutability, declared.mutability) {
        (crate::types::Mutability::Const, crate::types::Mutability::Const) => valtype_matches_ctx(
            provided_types,
            provided_recs,
            &provided.val_type,
            declared_types,
            declared_recs,
            &declared.val_type,
        ),
        (crate::types::Mutability::Var, crate::types::Mutability::Var) => valtype_identical_ctx(
            provided_types,
            provided_recs,
            &provided.val_type,
            declared_types,
            declared_recs,
            &declared.val_type,
        ),
        _ => false,
    }
}

/// A link group: instances that can reach each other's funcref values by
/// instance id. Created per link cluster (e.g. one per spec test file).
pub type LinkGroup = alloc::collections::BTreeMap<
    u32,
    (
        alloc::rc::Rc<crate::lower::RegModule>,
        alloc::rc::Rc<core::cell::RefCell<Store>>,
    ),
>;

/// A thrown exception record. `tag` is `(instance_id, tag_idx)` — the
/// throwing frame's module-local tag index qualified by its instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ExceptionRecord {
    pub tag: (u32, u32),
    pub payload: Vec<Value>,
}

/// A reference type with concrete (indexed) references resolved to their
/// defining module's rolled closed forms — the structural form in which tag
/// types are compared at link time. Abstract heap types stay nominal; a
/// concrete `(ref $t)` carries the complete [`ClosedDefinedType`] of `$t`
/// (finality, declared supertypes, and comptype), so cross-module comparison
/// is the spec's canonical defined-type equivalence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandedVal {
    Num(crate::types::NumType),
    Vec(crate::types::VecType),
    /// `funcref` / `(ref null func)` — the abstract function type.
    FuncRef {
        nullable: bool,
    },
    /// `externref` / `(ref null extern)`.
    ExternRef {
        nullable: bool,
    },
    /// `exnref` / `(ref null exn)`.
    ExnRef {
        nullable: bool,
    },
    /// `(ref $t)` where `$t` is a concrete defined type. The full rolled
    /// closed form is carried — finality, declared supertypes, and comptype —
    /// so that linking compares complete defined-type identity. In
    /// particular a non-final `(sub (struct (field i32)))` does NOT compare
    /// equal to a plain final `(struct (field i32))`, even though both are
    /// struct-typed with the same fields.
    ConcreteDefined {
        nullable: bool,
        def: ClosedDefinedType,
    },
}

/// A function type with concrete references expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedFunc {
    pub params: Vec<ExpandedVal>,
    pub results: Vec<ExpandedVal>,
}

/// A struct/array field with concrete references expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedField {
    pub mutable: bool,
    pub storage: ExpandedStorage,
}

/// A storage type with concrete references expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandedStorage {
    Val(ExpandedVal),
    I8,
    I16,
}

/// A composite type member of a rec group, expanded with its kind intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandedComp {
    Func(ExpandedFunc),
    Struct(Vec<ExpandedField>),
    Array(ExpandedField),
}

/// Rec-group provenance of a tag type, three-valued so that a FAILED
/// expansion (cycle or nested rec reference among the group's members)
/// never masquerades as "bare".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecProvenance {
    /// The tag type is outside any rec group.
    Bare,
    /// The tag type sits at `position` in a group with these expanded
    /// member signatures.
    Group(Vec<ExpandedComp>, u32),
    /// The group's members did not fully expand; never link-compatible
    /// (conservative rejection pending GC-era canonicalization).
    Unresolvable,
}

/// Expand a composite type with its kind intact. Concrete references now
/// carry their full rolled closed form, so this only fails on a genuinely
/// unresolvable reference (unknown index or an external cycle, impossible
/// for a validated module).
fn expand_comp(
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    comp: &CompType,
) -> Option<ExpandedComp> {
    Some(match comp {
        CompType::Func(func) => ExpandedComp::Func(expand_func(types, rec_groups, func)?),
        CompType::Struct(fields) => ExpandedComp::Struct(expand_fields(types, rec_groups, fields)?),
        CompType::Array(field) => ExpandedComp::Array(expand_field(types, rec_groups, *field)?),
    })
}

/// Expand one function type.
fn expand_func(
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    ty: &crate::types::FuncType,
) -> Option<ExpandedFunc> {
    Some(ExpandedFunc {
        params: ty
            .params
            .iter()
            .map(|v| expand_val(types, rec_groups, v))
            .collect::<Option<_>>()?,
        results: ty
            .results
            .iter()
            .map(|v| expand_val(types, rec_groups, v))
            .collect::<Option<_>>()?,
    })
}

fn expand_fields(
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    fields: &[FieldType],
) -> Option<Vec<ExpandedField>> {
    fields
        .iter()
        .map(|field| expand_field(types, rec_groups, *field))
        .collect()
}

fn expand_field(
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    field: FieldType,
) -> Option<ExpandedField> {
    Some(ExpandedField {
        mutable: field.mutable,
        storage: match field.storage {
            StorageType::Val(v) => ExpandedStorage::Val(expand_val(types, rec_groups, &v)?),
            StorageType::I8 => ExpandedStorage::I8,
            StorageType::I16 => ExpandedStorage::I16,
        },
    })
}

fn expand_val(
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    v: &crate::types::ValType,
) -> Option<ExpandedVal> {
    use crate::types::{HeapType, RefType, ValType};
    match v {
        ValType::Num(n) => Some(ExpandedVal::Num(*n)),
        ValType::Vec(x) => Some(ExpandedVal::Vec(*x)),
        ValType::Ref(RefType::FuncRef) => Some(ExpandedVal::FuncRef { nullable: true }),
        ValType::Ref(RefType::ExternRef) => Some(ExpandedVal::ExternRef { nullable: true }),
        ValType::Ref(RefType::Typed { nullable, heap }) => match heap {
            HeapType::Func => Some(ExpandedVal::FuncRef {
                nullable: *nullable,
            }),
            HeapType::Extern => Some(ExpandedVal::ExternRef {
                nullable: *nullable,
            }),
            HeapType::Exn => Some(ExpandedVal::ExnRef {
                nullable: *nullable,
            }),
            // Carry the COMPLETE rolled closed form of the referenced defined
            // type — finality, declared supertypes, and comptype — not just
            // the expanded shape. This is what makes linking sensitive to
            // finality and to differing supertype lists (review r3b P1).
            // `ClosedDefinedType::of` resolves in-group references relative
            // and external references recursively, and detects cycles.
            HeapType::Type(idx) => ClosedDefinedType::of(types, rec_groups, *idx).map(|def| {
                ExpandedVal::ConcreteDefined {
                    nullable: *nullable,
                    def,
                }
            }),
            // GC abstract heap types are not part of the EH-era link
            // expansion; they are never produced by EH tag types.
            HeapType::Any
            | HeapType::None
            | HeapType::Eq
            | HeapType::I31
            | HeapType::Struct
            | HeapType::Array
            | HeapType::NoFunc
            | HeapType::NoExtern
            | HeapType::NoExn => None,
        },
    }
}

/// A resolved tag binding: the global identity of the tag plus its type.
#[derive(Debug, Clone, PartialEq)]
pub struct TagBinding {
    /// `(instance_id, module_local_tag_idx)` of the DEFINING instance.
    pub identity: (u32, u32),
    pub ty: FuncType,
    /// Parameter types with concrete references resolved to their full
    /// rolled closed forms against the defining module's types (`None`
    /// when expansion fails — an unknown index or an external cycle, which
    /// a validated module cannot produce). Comparing these forms is the
    /// spec's canonical defined-type equivalence, so finality and declared
    /// supertypes participate.
    pub expanded_params: Option<Vec<ExpandedVal>>,
    /// Rec-group provenance of the tag type itself.
    pub rec: RecProvenance,
    /// The tag's OWN referenced function type's complete closed form
    /// (finality + supertypes + comptype), recomputed from the defining
    /// module's serialized type section. `None` when expansion fails (an
    /// unknown index or an external cycle, which a validated module cannot
    /// produce). This is what distinguishes an explicit non-final
    /// `(sub (func))` tag type from a synthesized final `(func)`.
    pub closed: Option<ClosedDefinedType>,
}

/// Mutable state of an instantiated module.
///
/// Defined memories and globals only; the imported index-space prefix is
/// tracked so accesses to imported memories/globals can fail explicitly.
pub struct Store {
    memories: Vec<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>>,
    memory_types: Vec<MemType>,
    globals: Vec<alloc::rc::Rc<core::cell::Cell<Value>>>,
    global_types: Vec<GlobalType>,
    tables: Vec<alloc::rc::Rc<core::cell::RefCell<crate::runtime::Table>>>,
    table_types: Vec<TableType>,
    /// Element segment storage; `None` after the segment is dropped (or was
    /// active/declarative at instantiation).
    elements: core::cell::RefCell<Vec<Option<Vec<Value>>>>,
    /// Data segment storage; `None` after the segment is dropped (or was
    /// active at instantiation).
    data: core::cell::RefCell<Vec<Option<Vec<u8>>>>,
    /// Export declarations (from the lowered module).
    exports: Vec<crate::lower::RegExport>,
    /// This instance's id within its link group (0 when unlinked).
    instance_id: u32,
    /// Remaining instruction fuel; `None` = unlimited (default). Embedders
    /// running untrusted modules set a budget via `set_fuel`; the
    /// interpreter errors with `FuelExhausted` at zero.
    fuel: core::cell::Cell<Option<u64>>,
    /// The start function, deferred until [`Store::run_start`] is called
    /// (host functions must be registered before it runs).
    pending_start: Option<crate::types::FuncIdx>,
    /// The link group this instance belongs to, if any.
    link_group: Option<alloc::rc::Rc<core::cell::RefCell<LinkGroup>>>,
    /// Optional GPU backend for bulk SIMD offload (Borsalino Level 1).
    gpu: core::cell::RefCell<Option<alloc::boxed::Box<dyn GpuBackend>>>,
    /// Element count at or above which bulk SIMD work dispatches to GPU.
    offload_threshold: usize,
    /// Lazily compiled offload kernels, keyed by kernel name.
    offload_kernels: alloc::collections::BTreeMap<&'static str, GpuKernelId>,
    /// Function import declarations (from the lowered module).
    imported_funcs: Vec<crate::lower::RegImport>,
    /// Registered host functions, one slot per function import.
    host_funcs: Vec<Option<alloc::rc::Rc<core::cell::RefCell<HostFunction>>>>,
    imported_memories: Vec<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>>,
    imported_memory_types: Vec<MemType>,
    imported_global_values: Vec<alloc::rc::Rc<core::cell::Cell<Value>>>,
    imported_global_types: Vec<GlobalType>,
    imported_tables: Vec<alloc::rc::Rc<core::cell::RefCell<crate::runtime::Table>>>,
    imported_table_types: Vec<TableType>,
    imported_memory_count: u32,
    imported_global_count: u32,
    imported_table_count: u32,
    /// Shared exception arena: one `Rc` per link group (cloned into every
    /// member store at instantiation) or per standalone store. Indices in
    /// `Value::ExnRef` / `Throw { exn }` are arena-global within the group.
    exceptions: alloc::rc::Rc<core::cell::RefCell<Vec<ExceptionRecord>>>,
    /// Function types of registered tags, indexed by a module-agnostic
    /// tag id. Populated at instantiation (U4); used for host-side
    /// inspection and cross-instance matching.
    tag_types: Vec<FuncType>,
    /// Module-local tag index → resolved global identity and type.
    /// Imported tags first, then defined (mirroring the validator/lowerer).
    tag_map: Vec<TagBinding>,
    /// The never-free GC heap (slice 5). Store-global: all allocations in
    /// this instance go through it, and `GcStats` is logical accounting for
    /// the whole store. Per-instance isolation is not required by the spec.
    gc_heap: alloc::rc::Rc<core::cell::RefCell<GcHeap>>,
    /// GC heaps registered from provider stores, keyed by their unique id.
    /// A `GcRef` whose heap is neither this store's heap nor in this list is
    /// a clean resolution error, never a misindexed local object.
    foreign_heaps: Vec<(HeapId, alloc::rc::Rc<core::cell::RefCell<GcHeap>>)>,
    /// Module-local defined-type index → heap-interned GC layout id,
    /// resolved once at instantiation so execution never allocates to build
    /// a `GcTypeKind` or a closed form. `None` for function types.
    gc_type_ids: Vec<Option<GcTypeId>>,
    /// Canonical type indices for this module's type section, so that two
    /// `FuncType`s expressed in the SAME index space can be compared by
    /// canonical identity without re-deriving closed forms (host-function
    /// registration may see several declarations of one import name whose
    /// local indices differ but whose types are `match_deftype`-equivalent).
    func_canon: crate::types::TypeCanon,
    /// This module's type section and rec groups. A runtime `FuncRef` carries
    /// no type of its own: `type_of_ref (FuncRef f) = UseHT (Def
    /// (Func.type_of f))` (`runtime/instance.ml:48`), so `ref.test`/`ref.cast`
    /// with a CONCRETE target resolve both the function's declared type and
    /// the target through these tables (`defined_type_matches`).
    type_defs: Vec<crate::types::SubType>,
    rec_groups: Vec<(u32, u32)>,
    /// Declared type index of every function in this module's index space
    /// (imported funcs first, mirroring `RegModule.funcs[].idx`). For an
    /// imported function this is the IMPORT DECLARATION; the funcref type is
    /// the binding's instead ([`Store::func_origin_type`]), so this table is
    /// only the declaration fallback for an unbound import.
    func_type_idxs: Vec<crate::types::TypeIdx>,
}

impl Store {
    /// Instantiate a lowered module with an empty import set. Modules with
    /// state imports (memories, globals, tables) need
    /// [`Store::instantiate_with_imports`].
    pub fn instantiate(module: &RegModule) -> Result<Self, RuntimeError> {
        Self::instantiate_with_imports(module, &Imports::new())
    }

    /// Instantiate and register the instance in a link group with the given
    /// instance id. Use for modules that link against each other so funcref
    /// values carry cross-instance identity.
    pub fn instantiate_linked(
        module: &Rc<RegModule>,
        imports: &Imports,
        group: &alloc::rc::Rc<core::cell::RefCell<LinkGroup>>,
        instance_id: u32,
        shared_exceptions: &Rc<core::cell::RefCell<Vec<ExceptionRecord>>>,
    ) -> Result<alloc::rc::Rc<core::cell::RefCell<Store>>, RuntimeError> {
        let (mut store, error) =
            Self::instantiate_internal(module, imports, instance_id, shared_exceptions.clone())?;
        store.link_group = Some(group.clone());
        // Cross-instance calls and the group-shared exception arena can carry
        // GC references from any already-registered member, so register every
        // member's heap as a foreign heap up front.
        store.register_group_heaps(group);
        let store = alloc::rc::Rc::new(core::cell::RefCell::new(store));
        // Register the instance even when instantiation failed partway:
        // spec semantics persist element segment writes into shared tables
        // from failed instantiations, and those functions remain callable.
        group
            .borrow_mut()
            .insert(instance_id, (module.clone(), store.clone()));
        register_group_heaps_symmetric(group);
        match error {
            Some(error) => Err(error),
            None => Ok(store),
        }
    }

    /// Instantiate a lowered module: resolve state imports eagerly from
    /// `imports`, zero memories to their minimum size, evaluate global
    /// initializers in declaration order, apply active data segments, and
    /// run the start function if present.
    pub fn instantiate_with_imports(
        module: &RegModule,
        imports: &Imports,
    ) -> Result<Self, RuntimeError> {
        let exceptions = Rc::new(core::cell::RefCell::new(Vec::new()));
        let (store, error) = Self::instantiate_internal(module, imports, 0, exceptions)?;
        match error {
            Some(error) => Err(error),
            None => Ok(store),
        }
    }

    /// Early-phase failures (import resolution, allocation, global init)
    /// produce no instance. Late-phase failures (segment application, start)
    /// produce a partially instantiated instance that callers may still
    /// register (spec: writes before the failure persist in shared tables).
    fn instantiate_internal(
        module: &RegModule,
        imports: &Imports,
        instance_id: u32,
        exceptions: Rc<core::cell::RefCell<Vec<ExceptionRecord>>>,
    ) -> Result<(Self, Option<RuntimeError>), RuntimeError> {
        // Resolve imported memories eagerly (spec limits matching).
        let mut imported_memories = Vec::with_capacity(module.imported_memories.len());
        let mut imported_memory_types = Vec::with_capacity(module.imported_memories.len());
        for declared in &module.imported_memories {
            let provider = imports
                .memories
                .iter()
                .find(|provider| {
                    provider.module == declared.module && provider.name == declared.name
                })
                .ok_or_else(|| unknown_import(&declared.module, &declared.name))?;
            // Import matching for linked entities uses the entity's CURRENT
            // size as the effective minimum (spec: external limits grow).
            let provided_limits = match &provider.shared {
                Some(shared) => Limits {
                    min: (shared.borrow().len() / PAGE_SIZE) as u64,
                    max: provider.ty.limits.max,
                    is64: provider.ty.limits.is64,
                    shared: provider.ty.limits.shared,
                },
                None => provider.ty.limits,
            };
            if !limits_match(&provided_limits, &declared.ty.limits) {
                return Err(import_type_mismatch(&declared.module, &declared.name));
            }
            let entity = match provider.shared.clone() {
                Some(shared) => shared,
                None => {
                    let bytes = usize::try_from(provider.ty.limits.min)
                        .ok()
                        .and_then(|pages| pages.checked_mul(PAGE_SIZE))
                        .and_then(|bytes| try_alloc_n(bytes, 0u8))
                        .ok_or_else(|| allocation_failed("imported memory"))?;
                    alloc::rc::Rc::new(core::cell::RefCell::new(bytes))
                }
            };
            imported_memories.push(entity);
            imported_memory_types.push(provider.ty);
        }

        // Resolve imported globals eagerly (exact type match).
        let mut imported_global_values = Vec::with_capacity(module.imported_globals.len());
        let mut imported_global_types = Vec::with_capacity(module.imported_globals.len());
        for declared in &module.imported_globals {
            let provider = imports
                .globals
                .iter()
                .find(|provider| {
                    provider.module == declared.module && provider.name == declared.name
                })
                .ok_or_else(|| unknown_import(&declared.module, &declared.name))?;
            let type_matches = match imports.type_contexts.get(&declared.module) {
                Some(provider_ctx) => global_type_matches_ctx(
                    &provider_ctx.types,
                    &provider_ctx.rec_groups,
                    &provider.ty,
                    &module.type_defs,
                    &module.rec_groups,
                    &declared.ty,
                ),
                None => global_type_matches(&provider.ty, &declared.ty),
            };
            if !type_matches {
                return Err(import_type_mismatch(&declared.module, &declared.name));
            }
            let entity = match (&provider.shared, provider.value) {
                (Some(shared), _) => shared.clone(),
                (None, Some(value)) => {
                    if value.val_type() != declared.ty.val_type {
                        return Err(import_type_mismatch(&declared.module, &declared.name));
                    }
                    alloc::rc::Rc::new(core::cell::Cell::new(value))
                }
                (None, None) => unreachable!("global provider sets value or shared"),
            };
            imported_global_values.push(entity);
            imported_global_types.push(provider.ty);
        }

        // Resolve imported tables eagerly (elem type exact, limits matching).
        let mut imported_tables = Vec::with_capacity(module.imported_tables.len());
        let mut imported_table_types = Vec::with_capacity(module.imported_tables.len());
        for declared in &module.imported_tables {
            let provider = imports
                .tables
                .iter()
                .find(|provider| {
                    provider.module == declared.module && provider.name == declared.name
                })
                .ok_or_else(|| unknown_import(&declared.module, &declared.name))?;
            let provided_limits = match &provider.shared {
                Some(shared) => Limits {
                    min: shared.borrow().len(),
                    max: provider.ty.limits.max,
                    is64: provider.ty.limits.is64,
                    shared: provider.ty.limits.shared,
                },
                None => provider.ty.limits,
            };
            let elem_matches = match imports.type_contexts.get(&declared.module) {
                Some(provider_ctx) => reftype_identical_ctx(
                    &provider_ctx.types,
                    &provider_ctx.rec_groups,
                    provider.ty.elem,
                    &module.type_defs,
                    &module.rec_groups,
                    declared.ty.elem,
                ),
                None => provider.ty.elem == declared.ty.elem,
            };
            if !elem_matches || !limits_match(&provided_limits, &declared.ty.limits) {
                return Err(import_type_mismatch(&declared.module, &declared.name));
            }
            let entity = match provider.shared.clone() {
                Some(shared) => shared,
                None => {
                    let values = crate::runtime::Table::new(
                        provider.ty.clone(),
                        table_null(provider.ty.clone()),
                    )
                    .ok_or_else(|| allocation_failed("imported table"))?;
                    alloc::rc::Rc::new(core::cell::RefCell::new(values))
                }
            };
            imported_tables.push(entity);
            imported_table_types.push(provider.ty.clone());
        }

        let imported_globals_plain: Vec<Value> = imported_global_values
            .iter()
            .map(|cell| cell.get())
            .collect();
        // Resolve GC aggregate metadata once, fallibly, BEFORE any global or
        // table state exists: heap-allocating const expressions (global
        // initializers, table init exprs, element segments) must follow the
        // same allocation discipline as execution — fallible, stats-counted,
        // and refusing cleanly with no partial instance.
        let mut gc_heap = GcHeap::new();
        let gc_type_ids = precompute_gc_type_ids(module, &mut gc_heap)?;
        let gc_heap = alloc::rc::Rc::new(core::cell::RefCell::new(gc_heap));
        let gc_heap_id = gc_heap.borrow().heap_id();

        let mut globals_plain: Vec<Value> = Vec::with_capacity(module.globals.len());
        let mut globals: Vec<alloc::rc::Rc<core::cell::Cell<Value>>> =
            Vec::with_capacity(module.globals.len());
        let mut global_types = Vec::with_capacity(module.globals.len());
        for global in &module.globals {
            let value = ConstEnv::init(
                &gc_heap,
                gc_heap_id,
                &gc_type_ids,
                instance_id,
                &imported_globals_plain,
                &globals_plain,
            )
            .eval(&global.init)?;
            globals_plain.push(value);
            globals.push(alloc::rc::Rc::new(core::cell::Cell::new(value)));
            global_types.push(crate::types::GlobalType {
                val_type: global.ty,
                mutability: if global.mutable {
                    crate::types::Mutability::Var
                } else {
                    crate::types::Mutability::Const
                },
            });
        }

        let mut memories = Vec::with_capacity(module.memories.len());
        for mem in &module.memories {
            let bytes = usize::try_from(mem.limits.min)
                .ok()
                .and_then(|pages| pages.checked_mul(PAGE_SIZE))
                .and_then(|bytes| try_alloc_n(bytes, 0u8))
                .ok_or_else(|| allocation_failed("memory"))?;
            memories.push(alloc::rc::Rc::new(core::cell::RefCell::new(bytes)));
        }
        let memory_types = module.memories.clone();

        let mut tables = Vec::with_capacity(module.tables.len());
        for table in &module.tables {
            let fill = match &table.init {
                Some(init) => {
                    let lowered =
                        crate::lower::lower_const_expr(init, 0).map_err(|_error| RuntimeError {
                            kind: RuntimeErrorKind::InvalidConstExpr,
                        })?;
                    ConstEnv::init(
                        &gc_heap,
                        gc_heap_id,
                        &gc_type_ids,
                        instance_id,
                        &imported_globals_plain,
                        &globals_plain,
                    )
                    .eval(&lowered)?
                }
                None => table_null(table.clone()),
            };
            let values = crate::runtime::Table::new(table.clone(), fill)
                .ok_or_else(|| allocation_failed("table"))?;
            tables.push(alloc::rc::Rc::new(core::cell::RefCell::new(values)));
        }
        let table_types = module.tables.clone();

        // Resolve tag imports eagerly, then bind defined tags to this
        // instance. Type compatibility compares RECURSIVELY EXPANDED
        // forms: concrete (indexed) references resolve against each
        // module's own type list, so identical structures at different
        // indices compare equal while actually-different signatures do
        // not — raw index comparison would be unsound. Rec-group
        // provenance participates with expanded member signatures.
        // Reference cycles expand to None and link-reject conservatively
        // (iso-recursive canonicalization is GC-era).
        let module_types = &module.types[..];
        let module_type_defs = &module.type_defs[..];
        let module_recs = &module.rec_groups[..];
        let rec_provenance = |raw: &Option<(Vec<SubType>, u32)>| -> RecProvenance {
            match raw {
                None => RecProvenance::Bare,
                Some((sig, pos)) => {
                    let expanded: Option<Vec<ExpandedComp>> = sig
                        .iter()
                        .map(|sub| expand_comp(module_type_defs, module_recs, &sub.comp))
                        .collect();
                    match expanded {
                        Some(sig) => RecProvenance::Group(sig, *pos),
                        None => RecProvenance::Unresolvable,
                    }
                }
            }
        };
        let mut tag_map = Vec::with_capacity(module.imported_tags.len() + module.tags.len());
        for declared in &module.imported_tags {
            let provider = imports
                .tags
                .iter()
                .find(|provider| {
                    provider.module == declared.module && provider.name == declared.name
                })
                .ok_or_else(|| unknown_import(&declared.module, &declared.name))?;
            let declared_params: Option<Vec<ExpandedVal>> = declared
                .ty
                .params
                .iter()
                .map(|v| expand_val(module_type_defs, module_recs, v))
                .collect();
            // The tag's OWN referenced function type: recompute its complete
            // closed form from the consumer's type section so finality and
            // declared supertypes participate (residual r3b follow-up).
            let declared_closed =
                ClosedDefinedType::of(module_type_defs, module_recs, declared.type_idx);
            let declared_rec = rec_provenance(&declared.rec);
            // Bare types are implicit SINGLETON rec groups in the
            // reference interpreter's canonicalization: a bare type and
            // a one-member rec group of the same shape are canonically
            // equal. Normalize both sides before comparing so that pair
            // links, while bare-vs-multi-member still differs by group
            // signature.
            let singleton = |params: &[ExpandedVal]| {
                RecProvenance::Group(
                    alloc::vec![ExpandedComp::Func(ExpandedFunc {
                        params: params.to_vec(),
                        results: Vec::new(),
                    })],
                    0,
                )
            };
            let provider_rec = match &provider.rec {
                RecProvenance::Bare => {
                    singleton(provider.expanded_params.as_deref().unwrap_or(&[]))
                }
                other => other.clone(),
            };
            let declared_rec = match &declared_rec {
                RecProvenance::Bare => singleton(declared_params.as_deref().unwrap_or(&[])),
                other => other.clone(),
            };
            let compatible = match (&provider.expanded_params, declared_params) {
                (Some(a), Some(b)) => a == &b,
                _ => false,
            } && match (&provider_rec, &declared_rec) {
                (RecProvenance::Group(sig_a, pos_a), RecProvenance::Group(sig_b, pos_b)) => {
                    sig_a == sig_b && pos_a == pos_b
                }
                _ => false,
            } && match (&provider.closed, &declared_closed) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            if !compatible {
                return Err(import_type_mismatch(&declared.module, &declared.name));
            }
            tag_map.push(TagBinding {
                identity: provider.identity,
                ty: provider.ty.clone(),
                expanded_params: provider.expanded_params.clone(),
                rec: provider.rec.clone(),
                closed: provider.closed.clone(),
            });
        }
        let imported_tag_count = module.imported_tags.len() as u32;
        for (i, type_idx) in module.tags.iter().enumerate() {
            let expanded_params: Option<Vec<ExpandedVal>> = module_types[type_idx.0 as usize]
                .params
                .iter()
                .map(|v| expand_val(module_type_defs, module_recs, v))
                .collect();
            let closed = ClosedDefinedType::of(module_type_defs, module_recs, *type_idx);
            let raw_rec = module
                .rec_groups
                .iter()
                .find(|&&(start, len)| type_idx.0 >= start && type_idx.0 < start + len)
                .map(|&(start, len)| {
                    (
                        module_type_defs[start as usize..(start + len) as usize].to_vec(),
                        type_idx.0 - start,
                    )
                });
            let rec = rec_provenance(&raw_rec);
            tag_map.push(TagBinding {
                identity: (instance_id, imported_tag_count + i as u32),
                ty: module_types[type_idx.0 as usize].clone(),
                expanded_params,
                rec,
                closed,
            });
        }

        // Resolve GC aggregate metadata once, fallibly: every defined
        // struct/array type is laid out and interned by its cross-module
        // closed form into this store's heap. Execution then allocates
        // without ever cloning a layout or closed form; a refusal here
        // produces no partial instance.
        let mut store = Self {
            memories,
            memory_types,
            globals,
            global_types,
            tables,
            table_types,
            elements: core::cell::RefCell::new(alloc::vec![None; module.elements.len()]),
            data: core::cell::RefCell::new(Vec::new()),
            exports: module.exports.clone(),
            instance_id,
            fuel: core::cell::Cell::new(None),
            pending_start: None,
            link_group: None,
            gpu: core::cell::RefCell::new(None),
            offload_threshold: DEFAULT_OFFLOAD_THRESHOLD,
            offload_kernels: alloc::collections::BTreeMap::new(),
            imported_funcs: module.imported_funcs.clone(),
            host_funcs: (0..module.imported_funcs.len()).map(|_| None).collect(),
            imported_memories,
            imported_memory_types,
            imported_global_values,
            imported_global_types,
            imported_tables,
            imported_table_types,
            imported_memory_count: module.imported_memory_count,
            imported_global_count: module.imported_global_count,
            imported_table_count: module.imported_table_count,
            exceptions,
            tag_types: Vec::new(),
            tag_map,
            gc_heap,
            foreign_heaps: register_provider_heaps(&imports.provider_heaps),
            gc_type_ids,
            func_canon: crate::types::TypeCanon::new(&module.type_defs, &module.rec_groups),
            type_defs: module.type_defs.clone(),
            rec_groups: module.rec_groups.clone(),
            func_type_idxs: func_type_idxs_of(module),
        };

        // Register defined-tag types (module-agnostic tag ids).
        for type_idx in &module.tags {
            if let Some(ty) = module.types.get(type_idx.0 as usize) {
                store.register_tag_type(ty.clone());
            }
        }

        let mut instantiation_error: Option<RuntimeError> = None;

        // Element segments: active ones are written into their tables (and
        // dropped); passive ones are retained for `table.init`. On the first
        // failure, remaining segments are skipped but earlier writes persist
        // (spec: writes before the failure persist in shared tables).
        for (idx, segment) in module.elements.iter().enumerate() {
            if instantiation_error.is_some() {
                break;
            }
            // Element values: `ref.func`/`global.get`/null are cheap, while
            // general const expressions (GC allocations) are evaluated
            // against the store's heap with the same allocation discipline
            // as execution.
            let mut values: Vec<Value> = Vec::with_capacity(segment.values.len());
            let mut value_error: Option<RuntimeError> = None;
            {
                let env = ConstEnv::from_store(&store);
                for value in &segment.values {
                    match value {
                        RegElemValue::FuncRef(func) => {
                            values.push(Value::FuncRef(Some((instance_id, func.0))))
                        }
                        RegElemValue::GlobalGet(global) => {
                            let idx = global.0 as usize;
                            if idx < store.imported_global_count as usize {
                                values.push(
                                    store
                                        .imported_global_values
                                        .get(idx)
                                        .expect("imported globals resolved")
                                        .get(),
                                );
                            } else {
                                values.push(
                                    store
                                        .globals
                                        .get(idx - store.imported_global_count as usize)
                                        .expect("defined globals initialized")
                                        .get(),
                                );
                            }
                        }
                        RegElemValue::Null => values.push(Value::FuncRef(None)),
                        RegElemValue::Expr(expr) => match env.eval(expr) {
                            Ok(value) => values.push(value),
                            Err(error) => {
                                value_error = Some(error);
                                break;
                            }
                        },
                    }
                }
            }
            if let Some(error) = value_error {
                instantiation_error = Some(error);
                break;
            }
            match &segment.mode {
                RegElementMode::Active { table, offset } => {
                    let offset = match ConstEnv::from_store(&store).eval(offset) {
                        Ok(offset) => offset,
                        Err(error) => {
                            instantiation_error = Some(error);
                            break;
                        }
                    };
                    let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
                    let offset = match (is64, offset) {
                        (false, Value::I32(offset)) => u64::from(offset as u32),
                        (true, Value::I64(offset)) => offset as u64,
                        _ => {
                            instantiation_error = Some(RuntimeError {
                                kind: RuntimeErrorKind::InvalidConstExpr,
                            });
                            break;
                        }
                    };
                    let applied = store
                        .with_table_mut(table.0, |target| {
                            if target.write_slice(offset, &values) {
                                Ok(())
                            } else {
                                Err(trap(RuntimeTrap::OutOfBoundsTableAccess))
                            }
                        })
                        .ok_or(RuntimeError {
                            kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                        })
                        .and_then(|result| result);
                    if let Err(error) = applied {
                        instantiation_error = Some(error);
                        break;
                    }
                }
                RegElementMode::Passive => {
                    store.elements.borrow_mut()[idx] = Some(values);
                }
                RegElementMode::Dropped => {}
            }
        }

        for segment in &module.data {
            if instantiation_error.is_some() {
                break;
            }
            match &segment.mode {
                RegDataMode::Active { memory, offset } => {
                    let offset = match ConstEnv::from_store(&store).eval(offset) {
                        Ok(offset) => offset,
                        Err(error) => {
                            instantiation_error = Some(error);
                            break;
                        }
                    };
                    let is64 = store.memory_type(memory.0).is_some_and(|ty| ty.limits.is64);
                    let offset = match (is64, offset) {
                        (false, Value::I32(offset)) => u64::from(offset as u32),
                        (true, Value::I64(offset)) => offset as u64,
                        _ => {
                            instantiation_error = Some(RuntimeError {
                                kind: RuntimeErrorKind::InvalidConstExpr,
                            });
                            break;
                        }
                    };
                    let applied = store
                        .with_memory_mut(memory.0, |mem| {
                            let Some(end) = offset.checked_add(segment.bytes.len() as u64) else {
                                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                            };
                            if end > mem.len() as u64 {
                                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                            }
                            let (start, end) = (offset as usize, end as usize);
                            mem[start..end].copy_from_slice(&segment.bytes);
                            Ok(())
                        })
                        .ok_or(RuntimeError {
                            kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
                        })
                        .and_then(|result| result);
                    if let Err(error) = applied {
                        instantiation_error = Some(error);
                        break;
                    }
                    store.data.borrow_mut().push(None);
                }
                RegDataMode::Passive => {
                    store.data.borrow_mut().push(Some(segment.bytes.clone()));
                }
            }
        }

        // The start function is deferred to [`Store::run_start`] so embedders
        // can register host functions first (skipped when instantiation
        // already failed).
        if instantiation_error.is_none() {
            store.pending_start = module.start;
        }

        Ok((store, instantiation_error))
    }

    /// Run the deferred start function, if any. Call after registering host
    /// functions; safe to call multiple times (runs at most once).
    pub fn run_start(&mut self, module: &RegModule) -> Result<(), RuntimeError> {
        let Some(start_idx) = self.pending_start.take() else {
            return Ok(());
        };
        if start_idx.0 < module.imported_func_count {
            self.call_host(start_idx.0, &[])?;
            return Ok(());
        }
        let func = module
            .funcs
            .iter()
            .find(|func| func.idx == start_idx)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: start_idx.0 },
            })?;
        crate::runtime::execute_func_in(Some(module), Some(self), func, &[], 0).map_err(
            |error| match error.kind {
                RuntimeErrorKind::Throw { exn } => RuntimeError {
                    kind: RuntimeErrorKind::UncaughtException { exn: Some(exn) },
                },
                _ => error,
            },
        )?;
        Ok(())
    }

    /// The shared handle of a memory by index-space index (imported first).
    ///
    /// Exposed so host-module crates (e.g. the GPU host module) can capture a
    /// guest-memory handle at registration time and read/write it from inside
    /// host-function closures. The handle stays valid across `memory.grow`,
    /// which mutates the `Vec` in place rather than replacing it.
    pub fn shared_memory(&self, idx: u32) -> Option<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>> {
        let idx = idx as usize;
        if idx < self.imported_memory_count as usize {
            return self.imported_memories.get(idx).cloned();
        }
        self.memories
            .get(idx - self.imported_memory_count as usize)
            .cloned()
    }

    /// The declared type of a memory by index-space index (imported first).
    pub(crate) fn memory_type(&self, idx: u32) -> Option<&MemType> {
        let idx = idx as usize;
        if idx < self.imported_memory_count as usize {
            return self.imported_memory_types.get(idx);
        }
        self.memory_types
            .get(idx - self.imported_memory_count as usize)
    }

    /// Read a global by index-space index (imported first).
    pub(crate) fn global(&self, idx: u32) -> Option<Value> {
        self.shared_global(idx).map(|cell| cell.get())
    }

    /// Write a global by index-space index (imported first).
    pub(crate) fn set_global(&self, idx: u32, value: Value) -> Option<()> {
        let cell = self.shared_global(idx)?;
        cell.set(value);
        Some(())
    }

    /// The shared cell of a global by index-space index (imported first).
    pub(crate) fn shared_global(&self, idx: u32) -> Option<alloc::rc::Rc<core::cell::Cell<Value>>> {
        let idx = idx as usize;
        if idx < self.imported_global_count as usize {
            return self.imported_global_values.get(idx).cloned();
        }
        self.globals
            .get(idx - self.imported_global_count as usize)
            .cloned()
    }

    /// The declared type of a global by index-space index (imported first).
    pub(crate) fn global_type(&self, idx: u32) -> Option<&GlobalType> {
        let idx = idx as usize;
        if idx < self.imported_global_count as usize {
            return self.imported_global_types.get(idx);
        }
        self.global_types
            .get(idx - self.imported_global_count as usize)
    }

    /// Read a table by index-space index (imported first) via closure.
    pub(crate) fn with_table<R>(
        &self,
        idx: u32,
        f: impl FnOnce(&crate::runtime::Table) -> R,
    ) -> Option<R> {
        let shared = self.shared_table(idx)?;
        let table = shared.borrow();
        Some(f(&table))
    }

    /// Mutate a table by index-space index (imported first) via closure.
    pub(crate) fn with_table_mut<R>(
        &self,
        idx: u32,
        f: impl FnOnce(&mut crate::runtime::Table) -> R,
    ) -> Option<R> {
        let shared = self.shared_table(idx)?;
        let mut table = shared.borrow_mut();
        Some(f(&mut table))
    }

    /// The shared handle of a table by index-space index (imported first).
    pub(crate) fn shared_table(
        &self,
        idx: u32,
    ) -> Option<alloc::rc::Rc<core::cell::RefCell<crate::runtime::Table>>> {
        let idx = idx as usize;
        if idx < self.imported_table_count as usize {
            return self.imported_tables.get(idx).cloned();
        }
        self.tables
            .get(idx - self.imported_table_count as usize)
            .cloned()
    }

    /// The declared type of a table by index-space index (imported first).
    pub(crate) fn table_type(&self, idx: u32) -> Option<&TableType> {
        let idx = idx as usize;
        if idx < self.imported_table_count as usize {
            return self.imported_table_types.get(idx);
        }
        self.table_types
            .get(idx - self.imported_table_count as usize)
    }

    /// An exported function by name.
    pub fn export_func(&self, name: &str) -> Option<FuncIdx> {
        self.exports.iter().find_map(|export| match export.desc {
            crate::lower::RegExportDesc::Func(idx) if export.name == name => Some(idx),
            _ => None,
        })
    }

    /// An exported tag by name: its module-local tag index (resolvable
    /// through [`Store::tag_binding`] for its identity and type).
    pub fn export_tag(&self, name: &str) -> Option<u32> {
        self.exports.iter().find_map(|export| match export.desc {
            crate::lower::RegExportDesc::Tag(idx) if export.name == name => Some(idx.0),
            _ => None,
        })
    }

    /// An exported memory by name: its declared type and shared handle.
    pub fn export_memory(
        &self,
        name: &str,
    ) -> Option<(MemType, alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>)> {
        let idx = self.exports.iter().find_map(|export| match export.desc {
            crate::lower::RegExportDesc::Mem(idx) if export.name == name => Some(idx),
            _ => None,
        })?;
        Some((*self.memory_type(idx.0)?, self.shared_memory(idx.0)?))
    }

    /// An exported global by name: its declared type and shared cell.
    pub fn export_global(
        &self,
        name: &str,
    ) -> Option<(GlobalType, alloc::rc::Rc<core::cell::Cell<Value>>)> {
        let idx = self.exports.iter().find_map(|export| match export.desc {
            crate::lower::RegExportDesc::Global(idx) if export.name == name => Some(idx),
            _ => None,
        })?;
        Some((*self.global_type(idx.0)?, self.shared_global(idx.0)?))
    }

    /// An exported table by name: its declared type and shared handle.
    pub fn export_table(
        &self,
        name: &str,
    ) -> Option<(
        TableType,
        alloc::rc::Rc<core::cell::RefCell<crate::runtime::Table>>,
    )> {
        let idx = self.exports.iter().find_map(|export| match export.desc {
            crate::lower::RegExportDesc::Table(idx) if export.name == name => Some(idx),
            _ => None,
        })?;
        Some((self.table_type(idx.0)?.clone(), self.shared_table(idx.0)?))
    }

    /// Read a retained element segment by index via closure.
    pub(crate) fn with_elem<R>(
        &self,
        idx: u32,
        f: impl FnOnce(Option<&Vec<Value>>) -> R,
    ) -> Option<R> {
        let elements = self.elements.borrow();
        let slot = elements.get(idx as usize)?;
        Some(f(slot.as_ref()))
    }

    /// Drop an element segment's storage.
    pub(crate) fn drop_elem(&self, idx: u32) -> Option<()> {
        let mut elements = self.elements.borrow_mut();
        let slot = elements.get_mut(idx as usize)?;
        *slot = None;
        Some(())
    }

    /// Read a retained data segment by index via closure.
    pub(crate) fn with_data<R>(
        &self,
        idx: u32,
        f: impl FnOnce(Option<&Vec<u8>>) -> R,
    ) -> Option<R> {
        let data = self.data.borrow();
        let slot = data.get(idx as usize)?;
        Some(f(slot.as_ref()))
    }

    /// Drop a data segment's storage.
    pub(crate) fn drop_data(&self, idx: u32) -> Option<()> {
        let mut data = self.data.borrow_mut();
        let slot = data.get_mut(idx as usize)?;
        *slot = None;
        Some(())
    }

    /// Read a global's current value by index-space index (embedding access).
    pub fn get_global(&self, idx: u32) -> Option<Value> {
        self.global(idx)
    }

    /// A logical snapshot of the GC heap's accounting.
    pub fn gc_stats(&self) -> GcStats {
        self.gc_heap.borrow().stats()
    }

    /// Set (or clear) the engine-level cap on the store's GC heap's
    /// LOGICAL live bytes: requests that would exceed it refuse as
    /// `ResourceLimitExceeded` with the heap untouched — portable
    /// structured refusal, independent of platform rlimits (macOS does
    /// not enforce RLIMIT_AS usefully).
    pub fn set_gc_heap_byte_limit(&self, limit: Option<u64>) {
        self.gc_heap.borrow_mut().set_byte_limit(limit);
    }

    /// A GC allocation snapshot for `allocations_since`.
    pub fn gc_snapshot(&self) -> GcSnapshot {
        self.gc_heap.borrow().snapshot()
    }

    /// Object-table insertions since `snapshot`.
    pub fn gc_allocations_since(&self, snapshot: GcSnapshot) -> u64 {
        self.gc_heap.borrow().allocations_since(snapshot)
    }

    /// Shared handle of the GC heap.
    pub fn gc_heap(&self) -> alloc::rc::Rc<core::cell::RefCell<GcHeap>> {
        self.gc_heap.clone()
    }

    /// Read the GC heap via closure.
    pub fn with_gc_heap<R>(&self, f: impl FnOnce(&GcHeap) -> R) -> R {
        let heap = self.gc_heap.borrow();
        f(&heap)
    }

    /// Mutate the GC heap via closure.
    pub fn with_gc_heap_mut<R>(&self, f: impl FnOnce(&mut GcHeap) -> R) -> R {
        let mut heap = self.gc_heap.borrow_mut();
        f(&mut heap)
    }

    /// This store's own GC heap identity.
    pub fn gc_heap_id(&self) -> HeapId {
        self.gc_heap.borrow().heap_id()
    }

    /// The heap-interned layout id for the store's module-local defined-type
    /// index, or a clean `UnknownType` when the index names no GC aggregate
    /// (out of range or a function type). Resolved at instantiation; never
    /// allocates.
    pub(crate) fn gc_type_id(&self, type_idx: TypeIdx) -> Result<GcTypeId, RuntimeError> {
        self.gc_type_ids
            .get(type_idx.0 as usize)
            .and_then(|slot| *slot)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownType {
                    type_idx: type_idx.0,
                },
            })
    }

    /// Resolve a heap identity to a heap handle: this store's own heap, a
    /// registered foreign heap, or `None`. Never panics on an unknown id.
    pub fn heap_handle(&self, heap: HeapId) -> Option<alloc::rc::Rc<core::cell::RefCell<GcHeap>>> {
        if self.gc_heap.borrow().heap_id() == heap {
            return Some(self.gc_heap.clone());
        }
        self.foreign_heaps
            .iter()
            .find(|(id, _)| *id == heap)
            .map(|(_, handle)| handle.clone())
    }

    /// Read a (possibly foreign) GC heap via closure. `None` means the
    /// reference's heap was never registered with this store.
    pub fn with_heap<R>(&self, heap: HeapId, f: impl FnOnce(&GcHeap) -> R) -> Option<R> {
        let handle = self.heap_handle(heap)?;
        let heap = handle.borrow();
        Some(f(&heap))
    }

    /// Mutate a (possibly foreign) GC heap via closure. `None` means the
    /// reference's heap was never registered with this store.
    pub fn with_heap_mut<R>(&self, heap: HeapId, f: impl FnOnce(&mut GcHeap) -> R) -> Option<R> {
        let handle = self.heap_handle(heap)?;
        let mut heap = handle.borrow_mut();
        Some(f(&mut heap))
    }

    /// Register every already-instantiated link-group member's GC heap as a
    /// foreign heap (deduplicated). Members added later are not retrograded;
    /// callers that link a provider after this store should also register its
    /// heap through [`Imports::provider_heap`].
    /// Register every group member's GC heap on every other member.
    ///
    /// A member instantiated earlier cannot know the heaps of peers created
    /// later, yet GC references cross instance boundaries in BOTH directions
    /// (a consumer-built struct passed to an imported function, or a
    /// provider-built struct stored in a shared table). Re-registration is
    /// idempotent and by heap identity.
    fn register_group_heaps(&mut self, group: &alloc::rc::Rc<core::cell::RefCell<LinkGroup>>) {
        let heaps: Vec<alloc::rc::Rc<core::cell::RefCell<GcHeap>>> = group
            .borrow()
            .iter()
            .filter_map(|(_, (_, store))| store.try_borrow().ok().map(|s| s.gc_heap.clone()))
            .collect();
        for heap in heaps {
            let id = heap.borrow().heap_id();
            if id != self.gc_heap.borrow().heap_id()
                && !self.foreign_heaps.iter().any(|(known, _)| *known == id)
            {
                self.foreign_heaps.push((id, heap));
            }
        }
    }

    /// Read a memory's bytes by index-space index via closure (embedding
    /// access; memories are shared handles, so direct slices are not
    /// available).
    pub fn with_memory<R>(&self, idx: u32, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let shared = self.shared_memory(idx)?;
        let mem = shared.borrow();
        Some(f(&mem))
    }

    /// Mutate a memory's bytes by index-space index via closure (embedding
    /// access).
    pub fn with_memory_mut<R>(&self, idx: u32, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
        let shared = self.shared_memory(idx)?;
        let mut mem = shared.borrow_mut();
        Some(f(&mut mem))
    }

    /// Install a GPU backend for bulk SIMD offload.
    pub fn set_gpu(&mut self, backend: alloc::boxed::Box<dyn GpuBackend>) {
        *self.gpu.borrow_mut() = Some(backend);
    }

    /// Remove the installed GPU backend, if any.
    pub fn clear_gpu(&mut self) {
        *self.gpu.borrow_mut() = None;
    }

    /// Whether a GPU backend is installed.
    pub fn has_gpu(&self) -> bool {
        self.gpu.borrow().is_some()
    }

    /// Mutable access to the installed GPU backend via closure.
    pub fn with_gpu_mut<R>(&self, f: impl FnOnce(&mut dyn GpuBackend) -> R) -> Option<R> {
        let mut gpu = self.gpu.borrow_mut();
        let backend = gpu.as_mut()?;
        Some(f(&mut **backend))
    }

    /// Register a host function for a function import `(module, name)`.
    ///
    /// Fails with [`RuntimeErrorKind::UnknownImport`] when the module does
    /// not declare that import, or [`RuntimeErrorKind::ImportTypeMismatch`]
    /// when the signatures differ.
    pub fn register_host_func(
        &mut self,
        module: &str,
        name: &str,
        func: HostFunction,
    ) -> Result<(), RuntimeError> {
        // One host slot per DECLARED import: a module may import the same
        // (module, name) function more than once, and every declaration
        // gets its own slot — fill them all, type-checking each.
        let shared_ty = func.ty().clone();
        let shared = alloc::rc::Rc::new(core::cell::RefCell::new(func));
        // Host and declaration types live in the same index space, so
        // `match_deftype` canonical identity is the right comparison: a
        // module may import one `(module, name)` more than once with
        // `FuncType`s whose local indices differ but whose canonical types are
        // equal (`type-equivalence.wast`).
        let shared_token = crate::types::canonical_func_token(&self.func_canon, &shared_ty);
        let mut filled = 0;
        for pos in 0..self.imported_funcs.len() {
            let import = &self.imported_funcs[pos];
            if import.module != module || import.name != name {
                continue;
            }
            if shared_token != crate::types::canonical_func_token(&self.func_canon, &import.ty) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::ImportTypeMismatch {
                        module: module.into(),
                        name: name.into(),
                    },
                });
            }
            self.host_funcs[pos] = Some(shared.clone());
            filled += 1;
        }
        if filled == 0 {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownImport {
                    module: module.into(),
                    name: name.into(),
                },
            });
        }
        Ok(())
    }

    /// Bind a host function to ONE function-import declaration, addressed by
    /// its index-space function index.
    ///
    /// [`Store::register_host_func`] fills every declaration of
    /// `(module, name)` and requires them to share one canonically-equal
    /// declared type. A module may instead import the same extern several
    /// times with DIFFERENT declared types, each matched against the
    /// provider's type by `match_deftype` (`type-subtyping.wast` imports one
    /// export at `$t0`, `$t1`, … from a provider whose function has type
    /// `$t1`). The caller holds both type sections and performs that match
    /// (`types::defined_type_matches`); this binds the resolved slot.
    ///
    /// The bound [`HostFunction`] also carries the type a FUNCREF to this
    /// import reports: `runtime/instance.ml:48` derives it from the function
    /// instance (`Func.type_of f`), so a [`link_func`](crate::runtime::link_func)
    /// binding keeps the provider's defining type and a native host function
    /// reports its own `FuncType` — never the import declaration, which only
    /// constrained linking.
    pub fn bind_host_func(
        &mut self,
        func_idx: u32,
        func: HostFunction,
    ) -> Result<(), RuntimeError> {
        let Some(slot) = self.host_funcs.get_mut(func_idx as usize) else {
            let (module, name) = self
                .imported_func(func_idx)
                .map(|import| (import.module.clone(), import.name.clone()))
                .unwrap_or_default();
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownImport { module, name },
            });
        };
        *slot = Some(alloc::rc::Rc::new(core::cell::RefCell::new(func)));
        Ok(())
    }

    /// Whether the function `func` of `instance` matches the concrete heap
    /// type `target` (a module-local type index of THIS store's module).
    ///
    /// `ref.test`/`ref.cast` on a function reference compare the function's
    /// DECLARED defined type against the target with `match_deftype`:
    /// `runtime/instance.ml:48` (`type_of_ref (FuncRef f) = UseHT (Def
    /// (Func.type_of f))`), `valid/match.ml:92` (concrete-vs-concrete routes
    /// to `match_deftype`), `valid/match.ml:151-155` (closed-form identity, or
    /// the unrolled declared-supertype chain reaches the target).
    ///
    /// `Func.type_of f` is the type of the FUNCTION INSTANCE the reference
    /// denotes — for an imported function, the PROVIDER's defining function
    /// (`exec/eval.ml:632-634`: `ref.func x` resolves `func c.frame.inst x`),
    /// never the importing module's import declaration. The declaration only
    /// constrains linking (`eval.ml:1187`); it is not the reference's type.
    /// [`Store::func_origin_type`] follows that resolution.
    pub(crate) fn func_ref_matches_type(
        &self,
        instance: u32,
        func: u32,
        target: crate::types::TypeIdx,
    ) -> bool {
        use crate::types::ClosedDefinedType;
        let Some(declared) = ClosedDefinedType::of(&self.type_defs, &self.rec_groups, target)
        else {
            return false;
        };
        let Some(provided) = self.func_defining_type(instance, func) else {
            return false;
        };
        provided == declared || provided.is_subtype_of(&declared)
    }

    /// The closed defined type of the function at `(instance, func)`.
    ///
    /// This store's own instance resolves through [`Store::func_origin_type`]
    /// (imports follow their binding to the provider). Another instance of the
    /// link group resolves through ITS module and, when `func` is one of its
    /// imports, through that instance's own recorded binding — the import
    /// chain terminates at whichever function actually defines it.
    fn func_defining_type(
        &self,
        instance: u32,
        func: u32,
    ) -> Option<crate::types::ClosedDefinedType> {
        use crate::types::{ClosedDefinedType, FuncIdx};
        if instance == self.instance_id {
            return self.func_origin_type(func);
        }
        let module = self.group_module(instance)?;
        if func < module.imported_func_count {
            // Imports of another instance: ask that instance's store for the
            // binding it installed. Borrow opportunistically — execution holds
            // only shared borrows, but a mutable borrow would mean this
            // instance is mid-reconfiguration, where the declaration is the
            // only type information available.
            let store = {
                let group = self.link_group.as_ref()?;
                let group = group.borrow();
                group.get(&instance)?.1.clone()
            };
            return store
                .try_borrow()
                .ok()
                .and_then(|store| store.func_origin_type(func));
        }
        let type_idx = module
            .funcs
            .iter()
            .find(|defined| defined.idx == FuncIdx(func))
            .map(|defined| defined.type_idx)?;
        ClosedDefinedType::of(&module.type_defs, &module.rec_groups, type_idx)
    }

    /// The defined type entry that types the function at `func_idx` in THIS
    /// instance's index space, as the runtime derives a funcref's type
    /// (`runtime/instance.ml:48`).
    ///
    /// A defined function uses this module's type entry. An IMPORTED function
    /// resolves through the binding installed on it:
    ///
    /// - a [`link_func`](crate::runtime::link_func) bridge carries the
    ///   provider's defining type (resolved through the provider's own import
    ///   chain when the provider re-exports an import),
    /// - a native host function's type is the embedder's bare function type
    ///   (`host/spectest.ml:35` allocates host types as one final, supertype-free
    ///   member), so it compares as [`ClosedDefinedType::of_func_type`].
    ///
    /// An UNBOUND import has no function instance at runtime, and a native
    /// host function whose signature mentions a CONCRETE heap type has no
    /// closed form of its own (a bare `FuncType` cannot express module-local
    /// indices); the declaration is then the only available (and previously
    /// used) answer.
    pub(crate) fn func_origin_type(
        &self,
        func_idx: u32,
    ) -> Option<crate::types::ClosedDefinedType> {
        use crate::types::ClosedDefinedType;
        if func_idx < self.imported_funcs.len() as u32 {
            if let Some(host) = self
                .host_funcs
                .get(func_idx as usize)
                .and_then(|slot| slot.as_ref())
                .and_then(|host| host.try_borrow().ok())
            {
                if let Some(origin) = host.origin_type() {
                    return Some(origin.clone());
                }
                if let Some(host_ty) = ClosedDefinedType::of_func_type(host.ty()) {
                    return Some(host_ty);
                }
                // Concrete heap types in a bare host signature: fall through
                // to the declaration, the only type information available.
            }
        }
        self.func_type_idxs
            .get(func_idx as usize)
            .and_then(|&type_idx| {
                ClosedDefinedType::of(&self.type_defs, &self.rec_groups, type_idx)
            })
    }

    /// The lowered module of an instance registered in this store's link
    /// group. `None` when unlinked or when the instance is unknown.
    pub fn group_module(&self, instance_id: u32) -> Option<alloc::rc::Rc<RegModule>> {
        let group = self.link_group.as_ref()?;
        let group = group.borrow();
        group.get(&instance_id).map(|(module, _)| module.clone())
    }

    /// This instance's id within its link group (0 when unlinked).
    pub(crate) fn instance_id(&self) -> u32 {
        self.instance_id
    }

    /// Set the instruction fuel budget (`None` = unlimited, the default).
    ///
    /// Fuel is charged per executed instruction (and per block dispatch)
    /// across all calls made from this store. Cross-instance calls charge
    /// the callee's store, not the caller's — each instance has its own
    /// budget.
    pub fn set_fuel(&self, fuel: Option<u64>) {
        self.fuel.set(fuel);
    }

    /// Remaining fuel, or `None` when unlimited.
    pub fn fuel(&self) -> Option<u64> {
        self.fuel.get()
    }

    /// Charge one unit of fuel; `false` when the budget is exhausted.
    pub(crate) fn charge_fuel(&self) -> bool {
        let Some(remaining) = self.fuel.get() else {
            return true;
        };
        if remaining == 0 {
            return false;
        }
        self.fuel.set(Some(remaining - 1));
        true
    }

    /// Allocate an exception record in the arena, returning its index.
    pub fn allocate_exception(&self, tag: (u32, u32), payload: Vec<Value>) -> u32 {
        let mut exceptions = self.exceptions.borrow_mut();
        let idx = exceptions.len() as u32;
        exceptions.push(ExceptionRecord { tag, payload });
        idx
    }

    /// Clone the exception record at `idx`, or `None` when out of range.
    pub fn exception(&self, idx: u32) -> Option<ExceptionRecord> {
        self.exceptions.borrow().get(idx as usize).cloned()
    }

    /// Register a tag's function type, returning its module-agnostic tag id.
    pub fn register_tag_type(&mut self, ty: FuncType) -> u32 {
        let id = self.tag_types.len() as u32;
        self.tag_types.push(ty);
        id
    }

    /// The resolved binding for a module-local tag index, if any.
    pub fn tag_binding(&self, module_local_idx: u32) -> Option<&TagBinding> {
        self.tag_map.get(module_local_idx as usize)
    }

    /// The link group this instance belongs to, if any.
    pub(crate) fn link_group(&self) -> Option<&alloc::rc::Rc<core::cell::RefCell<LinkGroup>>> {
        self.link_group.as_ref()
    }

    /// Invoke the registered host function for an imported function index,
    /// failing with [`RuntimeErrorKind::UnknownImport`] when unregistered.
    pub(crate) fn call_host(
        &self,
        func_idx: u32,
        args: &[Value],
    ) -> Result<Vec<Value>, RuntimeError> {
        if self
            .host_funcs
            .get(func_idx as usize)
            .and_then(Option::as_ref)
            .is_none()
        {
            let (module, name) = self
                .imported_func(func_idx)
                .map(|import| (import.module.clone(), import.name.clone()))
                .unwrap_or_default();
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownImport { module, name },
            });
        }
        let host = self.host_funcs[func_idx as usize]
            .as_ref()
            .expect("registration checked above");
        // A host slot may be shared between duplicate imports of the same
        // (module, name): a host closure that re-enters the module and
        // calls through another slot bound to the same closure would
        // re-borrow the shared cell. Surface a fallible reentrancy error
        // instead of panicking.
        let mut host = host.try_borrow_mut().map_err(|_| RuntimeError {
            kind: RuntimeErrorKind::ReentrantStore,
        })?;
        host.call(args)
    }

    /// The import declaration for an imported function index.
    pub(crate) fn imported_func(&self, func_idx: u32) -> Option<&crate::lower::RegImport> {
        self.imported_funcs.get(func_idx as usize)
    }

    /// Set the element count at or above which bulk SIMD work dispatches
    /// to GPU (per-platform tuning).
    pub fn set_offload_threshold(&mut self, threshold: usize) {
        self.offload_threshold = threshold;
    }

    /// Bulk element-wise f32 addition over linear-memory regions:
    /// `out[i] = a[i] + b[i]` for `count` f32 elements (Borsalino Level 1).
    ///
    /// At or above the offload threshold with a GPU backend installed, the
    /// work dispatches to GPU; otherwise it executes on CPU. Memory 0.
    pub fn f32_add_region(
        &mut self,
        a_ptr: u32,
        b_ptr: u32,
        out_ptr: u32,
        count: usize,
    ) -> Result<(), RuntimeError> {
        let byte_len = count
            .checked_mul(4)
            .ok_or(trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
        let mem_len = self.with_memory(0, |mem| mem.len()).ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownMemory { memory: 0 },
        })? as u64;
        for ptr in [a_ptr, b_ptr, out_ptr] {
            let end = ptr as u64 + byte_len as u64;
            if end > mem_len {
                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
            }
        }

        if !self.has_gpu() || count < self.offload_threshold {
            // CPU path: element-wise add over the regions.
            self.with_memory_mut(0, |mem| {
                for i in 0..count {
                    let at = a_ptr as usize + i * 4;
                    let bt = b_ptr as usize + i * 4;
                    let ot = out_ptr as usize + i * 4;
                    let lhs =
                        f32::from_le_bytes(mem[at..at + 4].try_into().expect("width checked"));
                    let rhs =
                        f32::from_le_bytes(mem[bt..bt + 4].try_into().expect("width checked"));
                    mem[ot..ot + 4].copy_from_slice(&(lhs + rhs).to_le_bytes());
                }
            })
            .expect("memory 0 length checked above");
            return Ok(());
        }

        // GPU path: compile (once), upload regions, dispatch, read back.
        let kernel = match self.offload_kernels.get("f32_add") {
            Some(&kernel) => kernel,
            None => {
                let kernel = self
                    .with_gpu_mut(|gpu| gpu.compile("vadd", F32_ADD_WGSL))
                    .expect("gpu checked above")
                    .map_err(runtime_gpu_error)?;
                self.offload_kernels.insert("f32_add", kernel);
                kernel
            }
        };

        let mem = self.shared_memory(0).ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownMemory { memory: 0 },
        })?;
        let (a_bytes, b_bytes) = {
            let mem = mem.borrow();
            (
                mem[a_ptr as usize..a_ptr as usize + byte_len].to_vec(),
                mem[b_ptr as usize..b_ptr as usize + byte_len].to_vec(),
            )
        };
        let buf_a = self
            .with_gpu_mut(|gpu| gpu.create_buffer(&a_bytes))
            .expect("gpu checked above")
            .map_err(runtime_gpu_error)?;
        let buf_b = self
            .with_gpu_mut(|gpu| gpu.create_buffer(&b_bytes))
            .expect("gpu checked above")
            .map_err(runtime_gpu_error)?;
        let buf_out = self
            .with_gpu_mut(|gpu| gpu.create_buffer_uninit(byte_len))
            .expect("gpu checked above")
            .map_err(runtime_gpu_error)?;
        let workgroups = [count.div_ceil(256) as u32, 1, 1];
        self.with_gpu_mut(|gpu| {
            gpu.dispatch_verified(kernel, &[buf_a, buf_b, buf_out], workgroups, [256, 1, 1])
        })
        .expect("gpu checked above")
        .map_err(runtime_gpu_error)?;
        let result = self
            .with_gpu_mut(|gpu| gpu.read_buffer(buf_out))
            .expect("gpu checked above")
            .map_err(runtime_gpu_error)?;
        mem.borrow_mut()[out_ptr as usize..out_ptr as usize + byte_len]
            .copy_from_slice(&result[..byte_len]);
        Ok(())
    }
}

/// Map a GPU backend error into a runtime error.
fn runtime_gpu_error(error: GpuError) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::Gpu(error),
    }
}

/// Reads the global index space (`imported ++ defined`) for a const
/// expression. Before the `Store` exists (global initializers), only the
/// values initialized so far are visible; afterwards the store's cells are
/// read directly.
enum ConstGlobals<'a> {
    Init {
        imported: &'a [Value],
        defined: &'a [Value],
    },
    Store {
        imported: &'a [alloc::rc::Rc<core::cell::Cell<Value>>],
        defined: &'a [alloc::rc::Rc<core::cell::Cell<Value>>],
    },
}

impl ConstGlobals<'_> {
    fn get(&self, idx: u32) -> Option<Value> {
        match self {
            Self::Init { imported, defined } => {
                let idx = idx as usize;
                if idx < imported.len() {
                    imported.get(idx).copied()
                } else {
                    defined.get(idx - imported.len()).copied()
                }
            }
            Self::Store { imported, defined } => {
                let idx = idx as usize;
                if idx < imported.len() {
                    imported.get(idx).map(|cell| cell.get())
                } else {
                    defined.get(idx - imported.len()).map(|cell| cell.get())
                }
            }
        }
    }
}

/// The environment a const expression is evaluated in: the module's heap
/// (so heap-allocating forms use the instantiation-time type cache and the
/// never-free heap's fallible allocation discipline), the globals available
/// at this point, and the instance identity `ref.func` values carry.
struct ConstEnv<'a> {
    heap: &'a alloc::rc::Rc<core::cell::RefCell<GcHeap>>,
    heap_id: HeapId,
    gc_type_ids: &'a [Option<GcTypeId>],
    instance_id: u32,
    globals: ConstGlobals<'a>,
}

impl<'a> ConstEnv<'a> {
    /// The pre-store environment used while initializing defined globals.
    fn init(
        heap: &'a alloc::rc::Rc<core::cell::RefCell<GcHeap>>,
        heap_id: HeapId,
        gc_type_ids: &'a [Option<GcTypeId>],
        instance_id: u32,
        imported_globals: &'a [Value],
        defined_globals: &'a [Value],
    ) -> Self {
        Self {
            heap,
            heap_id,
            gc_type_ids,
            instance_id,
            globals: ConstGlobals::Init {
                imported: imported_globals,
                defined: defined_globals,
            },
        }
    }

    /// The post-instantiation environment: reads the store's own globals.
    fn from_store(store: &'a Store) -> Self {
        Self {
            heap: &store.gc_heap,
            heap_id: store.gc_heap.borrow().heap_id(),
            gc_type_ids: &store.gc_type_ids,
            instance_id: store.instance_id,
            globals: ConstGlobals::Store {
                imported: &store.imported_global_values,
                defined: &store.globals,
            },
        }
    }

    fn type_id(&self, type_idx: TypeIdx) -> Result<GcTypeId, RuntimeError> {
        self.gc_type_ids
            .get(type_idx.0 as usize)
            .and_then(|slot| *slot)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownType {
                    type_idx: type_idx.0,
                },
            })
    }

    /// Evaluate a lowered const expression to exactly one value. Heap forms
    /// allocate through the store's heap (fallible, stats-counted); a refusal
    /// or a malformed expression is a clean error with nothing published.
    fn eval(&self, expr: &[RegConstInstr]) -> Result<Value, RuntimeError> {
        let mut stack = ConstStack::new();
        for instr in expr {
            match *instr {
                RegConstInstr::I32Const(value) => stack.push(Value::I32(value))?,
                RegConstInstr::I64Const(value) => stack.push(Value::I64(value))?,
                RegConstInstr::F32Const(bits) => stack.push(Value::F32(f32::from_bits(bits)))?,
                RegConstInstr::F64Const(bits) => stack.push(Value::F64(f64::from_bits(bits)))?,
                RegConstInstr::V128Const(bytes) => stack.push(Value::V128(bytes))?,
                RegConstInstr::RefNull => stack.push(Value::FuncRef(None))?,
                RegConstInstr::RefNullOf(ref_type) => {
                    stack.push(crate::runtime::ref_null_value(&ref_type))?
                }
                RegConstInstr::RefNullGC(heap) => stack.push(crate::runtime::ref_null_value(
                    &RefType::from_parts(true, heap),
                ))?,
                RegConstInstr::RefFunc(func) => {
                    stack.push(Value::FuncRef(Some((self.instance_id, func.0))))?;
                }
                RegConstInstr::GlobalGet(global) => {
                    let value = self.globals.get(global.0).ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
                    })?;
                    stack.push(value)?;
                }
                RegConstInstr::I32Add => {
                    let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                    stack.push(Value::I32(lhs.wrapping_add(rhs)))?;
                }
                RegConstInstr::I32Sub => {
                    let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                    stack.push(Value::I32(lhs.wrapping_sub(rhs)))?;
                }
                RegConstInstr::I32Mul => {
                    let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                    stack.push(Value::I32(lhs.wrapping_mul(rhs)))?;
                }
                RegConstInstr::I64Add => {
                    let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                    stack.push(Value::I64(lhs.wrapping_add(rhs)))?;
                }
                RegConstInstr::I64Sub => {
                    let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                    stack.push(Value::I64(lhs.wrapping_sub(rhs)))?;
                }
                RegConstInstr::I64Mul => {
                    let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                    stack.push(Value::I64(lhs.wrapping_mul(rhs)))?;
                }
                RegConstInstr::RefI31 => {
                    let Some(Value::I32(value)) = stack.pop() else {
                        return Err(invalid_const_expr());
                    };
                    stack.push(Value::I31((value as u32 & 0x7FFF_FFFF) as i32))?;
                }
                RegConstInstr::StructNew { type_idx, default } => {
                    let type_id = self.type_id(type_idx)?;
                    // Copy the field layout fallibly: the struct's field count
                    // is module-controlled, so an infallible `collect()` would
                    // abort on allocator refusal.
                    let mut storages: Vec<StorageType> = Vec::new();
                    {
                        let heap = self.heap.borrow();
                        let Some(GcTypeKind::Struct(field_types)) = heap.type_kind(type_id) else {
                            return Err(invalid_const_expr());
                        };
                        storages
                            .try_reserve(field_types.len())
                            .map_err(|_| gc_const_allocation_failed())?;
                        storages.extend(field_types.iter().map(|field| field.storage));
                    }
                    let mut fields: Vec<Value> = Vec::new();
                    fields
                        .try_reserve(storages.len())
                        .map_err(|_| gc_const_allocation_failed())?;
                    if default {
                        for storage in &storages {
                            fields.push(default_for_storage(*storage));
                        }
                    } else {
                        // Operands arrive last-field-first: pop them in that
                        // order, then reverse in place (no replacement vec).
                        for _ in 0..storages.len() {
                            fields.push(stack.pop().ok_or_else(invalid_const_expr)?);
                        }
                        fields.reverse();
                    }
                    let id = self.heap.borrow_mut().alloc_struct_typed(type_id, fields)?;
                    stack.push(Value::GcRef {
                        heap: self.heap_id,
                        id,
                    })?;
                }
                RegConstInstr::ArrayNew { type_idx, default } => {
                    let type_id = self.type_id(type_idx)?;
                    let storage = {
                        let heap = self.heap.borrow();
                        match heap.type_kind(type_id) {
                            Some(GcTypeKind::Array(field)) => field.storage,
                            _ => return Err(invalid_const_expr()),
                        }
                    };
                    let Some(Value::I32(len)) = stack.pop() else {
                        return Err(invalid_const_expr());
                    };
                    let fill = if default {
                        default_for_storage(storage)
                    } else {
                        stack.pop().ok_or_else(invalid_const_expr)?
                    };
                    let id = self
                        .heap
                        .borrow_mut()
                        .alloc_array_typed(type_id, len as u32, fill)?;
                    stack.push(Value::GcRef {
                        heap: self.heap_id,
                        id,
                    })?;
                }
                RegConstInstr::ArrayNewFixed { type_idx, count } => {
                    let type_id = self.type_id(type_idx)?;
                    let mut elems: Vec<Value> = Vec::new();
                    elems
                        .try_reserve(count as usize)
                        .map_err(|_| gc_const_allocation_failed())?;
                    // Operands arrive last-element-first.
                    for _ in 0..count {
                        elems.push(stack.pop().ok_or_else(invalid_const_expr)?);
                    }
                    elems.reverse();
                    let id = self.heap.borrow_mut().alloc_fixed_typed(type_id, elems)?;
                    stack.push(Value::GcRef {
                        heap: self.heap_id,
                        id,
                    })?;
                }
                RegConstInstr::ExternConvert { direction } => {
                    let value = stack.pop().ok_or_else(invalid_const_expr)?;
                    let converted = match direction {
                        crate::lower::ExternConvert::AnyToExtern => {
                            crate::runtime::extern_convert_any(value)?
                        }
                        crate::lower::ExternConvert::ExternToAny => {
                            crate::runtime::any_convert_extern(value)?
                        }
                    };
                    stack.push(converted)?;
                }
            }
        }

        if stack.len() != 1 {
            return Err(invalid_const_expr());
        }
        Ok(*stack.get(0).ok_or_else(invalid_const_expr)?)
    }
}

/// The const-expression evaluator's operand stack.
///
/// Every push reserves fallibly. A const expression is module-controlled
/// (`struct.new`/`array.new_fixed` operand lists grow with the declared
/// field/element count, and every nested form pushes), so an infallible
/// `Vec` growth would abort the process under allocator pressure instead of
/// refusing with a clean [`RuntimeErrorKind::ResourceLimitExceeded`].
struct ConstStack {
    values: Vec<Value>,
}

impl ConstStack {
    fn new() -> Self {
        Self { values: Vec::new() }
    }

    fn push(&mut self, value: Value) -> Result<(), RuntimeError> {
        self.values
            .try_reserve(1)
            .map_err(|_| gc_const_allocation_failed())?;
        self.values.push(value);
        Ok(())
    }

    fn pop(&mut self) -> Option<Value> {
        self.values.pop()
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn get(&self, index: usize) -> Option<&Value> {
        self.values.get(index)
    }
}

fn invalid_const_expr() -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::InvalidConstExpr,
    }
}

fn gc_const_allocation_failed() -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::ResourceLimitExceeded {
            what: "gc const expression",
        },
    }
}

fn pop_i32_pair(stack: &mut ConstStack) -> Result<(i32, i32), RuntimeError> {
    let (Some(Value::I32(rhs)), Some(Value::I32(lhs))) = (stack.pop(), stack.pop()) else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    };
    Ok((lhs, rhs))
}

fn pop_i64_pair(stack: &mut ConstStack) -> Result<(i64, i64), RuntimeError> {
    let (Some(Value::I64(rhs)), Some(Value::I64(lhs))) = (stack.pop(), stack.pop()) else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    };
    Ok((lhs, rhs))
}

impl core::fmt::Debug for Store {
    /// Non-recursive: the store links into a group whose members hold
    /// stores back (Store -> LinkGroup -> Store), so a derived Debug
    /// overflows the stack on any linked instance. Summarize instead.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Store")
            .field("instance_id", &self.instance_id)
            .field("memories", &self.memories.len())
            .field("globals", &self.globals.len())
            .field("tables", &self.tables.len())
            .field("imported_funcs", &self.imported_funcs.len())
            .field("exports", &self.exports.len())
            .field("tags", &self.tag_map.len())
            .field(
                "link_group",
                &self.link_group.as_ref().map(|g| g.borrow().len()),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod host_reentrancy_tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    /// Review r1 P1-2: two imports of the same (module, name) host share
    /// one host cell. A host closure that re-enters the module through
    /// the other slot must surface a fallible reentrancy error, not a
    /// `RefCell` panic.
    #[test]
    fn shared_host_slot_reentrancy_is_a_fallible_error() {
        use crate::binary::module::Module;
        let source = r#"
(module
  (import "h" "f" (func $a))
  (import "h" "f" (func $b))
  (func (export "outer") (call $a))
  (func (export "inner") (call $b)))"#;
        let buf = wast::parser::ParseBuffer::new(source).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        let module = alloc::rc::Rc::new(Module::decode(&bytes).unwrap().lower().unwrap());

        let group: Rc<RefCell<LinkGroup>> = Rc::new(RefCell::new(BTreeMap::new()));
        let arena: Rc<RefCell<Vec<ExceptionRecord>>> = Rc::new(RefCell::new(Vec::new()));
        let store = Store::instantiate_linked(&module, &Imports::new(), &group, 1, &arena).unwrap();

        // One host closure, shared across both import slots: calling the
        // slot-$b entry from inside the slot-$a invocation re-borrows the
        // shared cell.
        {
            let mut guard = store.borrow_mut();
            let ty = FuncType {
                params: Vec::new(),
                results: Vec::new(),
            };
            let store_capture = store.clone();
            let host = HostFunction::new(ty, move |_args| {
                // Slot 1 is the second `$b` import.
                let borrowed = store_capture.borrow();
                let result = borrowed.call_host(1, &[]);
                drop(borrowed);
                result.map(|_| Vec::new())
            });
            guard
                .register_host_func("h", "f", host)
                .expect("both slots registered");
        }

        let error = {
            let borrowed = store.borrow();
            let f = &module.funcs[0];
            crate::runtime::execute_func_in(Some(&module), Some(&borrowed), f, &[], 0).unwrap_err()
        };
        assert!(
            matches!(error.kind, RuntimeErrorKind::ReentrantStore),
            "expected ReentrantStore, got {error:?}"
        );
    }
}

#[cfg(test)]
mod threads_tests {
    use super::*;

    #[test]
    fn limits_match_shared_rules() {
        let unshared = Limits {
            min: 1,
            max: Some(2),
            is64: false,
            shared: false,
        };
        let shared = Limits {
            min: 1,
            max: Some(2),
            is64: false,
            shared: true,
        };
        // shared is part of the type: never links across, either direction
        assert!(!limits_match(&unshared, &shared));
        assert!(!limits_match(&shared, &unshared));
        // shared-to-shared links under the usual min/max rules
        assert!(limits_match(&shared, &shared));
        assert!(limits_match(
            &Limits {
                min: 2,
                max: Some(2),
                is64: false,
                shared: true
            },
            &shared
        ));
    }
}
