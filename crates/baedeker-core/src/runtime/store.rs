//! Mutable runtime state for an instantiated module: linear memories and
//! globals. See [Spec §4.4](https://webassembly.github.io/spec/core/exec/runtime.html).

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lower::{RegConstInstr, RegDataMode, RegElemValue, RegElementMode, RegModule};
use crate::runtime::gpu::{GpuBackend, GpuError, GpuKernelId};
use crate::runtime::host::HostFunction;
use crate::runtime::{RuntimeError, RuntimeErrorKind, RuntimeTrap, Value, trap};
use crate::types::{FuncIdx, GlobalType, Limits, MemType, TableType};

/// WGSL element-wise f32 add kernel for bulk SIMD offload.
const WGSL_F32_ADD: &str = r#"
@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(256)
fn vadd(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i < arrayLength(&out)) {
        out[i] = a[i] + b[i];
    }
}
"#;

/// Default element count at or above which bulk SIMD work dispatches to
/// GPU. Below this, CPU execution avoids dispatch overhead. Per-platform
/// tuning (unified-memory vs discrete GPU) adjusts this at runtime.
pub const DEFAULT_OFFLOAD_THRESHOLD: usize = 1024;

/// Size of one WebAssembly memory page in bytes.
pub const PAGE_SIZE: usize = 65536;

/// Entities provided by the host to satisfy a module's state imports
/// (memories, globals, tables), resolved eagerly at instantiation.
/// Function imports resolve separately and lazily via
/// [`Store::register_host_func`].
#[derive(Debug, Default)]
pub struct Imports {
    memories: Vec<MemoryProvider>,
    globals: Vec<GlobalProvider>,
    tables: Vec<TableProvider>,
}

impl Imports {
    /// An empty import set.
    pub fn new() -> Self {
        Self::default()
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
        shared: alloc::rc::Rc<core::cell::RefCell<Vec<Value>>>,
    ) -> Self {
        self.tables.push(TableProvider {
            module: module.into(),
            name: name.into(),
            ty,
            shared: Some(shared),
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
    shared: Option<alloc::rc::Rc<core::cell::RefCell<Vec<Value>>>>,
}

/// The null value for a table's element type.
fn table_null(ty: TableType) -> Value {
    match ty.elem {
        crate::types::RefType::ExternRef => Value::ExternRef(None),
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

/// Spec limits matching: provided limits must be at least as permissive as
/// declared import limits.
fn limits_match(provided: &Limits, declared: &Limits) -> bool {
    if provided.min < declared.min {
        return false;
    }
    match (provided.max, declared.max) {
        (_, None) => true,
        (Some(provided), Some(declared)) => provided <= declared,
        (None, Some(_)) => false,
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

/// A link group: instances that can reach each other's funcref values by
/// instance id. Created per link cluster (e.g. one per spec test file).
pub type LinkGroup = alloc::collections::BTreeMap<
    u32,
    (
        alloc::rc::Rc<crate::lower::RegModule>,
        alloc::rc::Rc<core::cell::RefCell<Store>>,
    ),
>;

/// Mutable state of an instantiated module.
///
/// Defined memories and globals only; the imported index-space prefix is
/// tracked so accesses to imported memories/globals can fail explicitly.
#[derive(Debug)]
pub struct Store {
    memories: Vec<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>>,
    memory_types: Vec<MemType>,
    globals: Vec<alloc::rc::Rc<core::cell::Cell<Value>>>,
    global_types: Vec<GlobalType>,
    tables: Vec<alloc::rc::Rc<core::cell::RefCell<Vec<Value>>>>,
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
    host_funcs: Vec<Option<core::cell::RefCell<HostFunction>>>,
    imported_memories: Vec<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>>,
    imported_memory_types: Vec<MemType>,
    imported_global_values: Vec<alloc::rc::Rc<core::cell::Cell<Value>>>,
    imported_global_types: Vec<GlobalType>,
    imported_tables: Vec<alloc::rc::Rc<core::cell::RefCell<Vec<Value>>>>,
    imported_table_types: Vec<TableType>,
    imported_memory_count: u32,
    imported_global_count: u32,
    imported_table_count: u32,
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
    ) -> Result<alloc::rc::Rc<core::cell::RefCell<Store>>, RuntimeError> {
        let (mut store, error) = Self::instantiate_internal(module, imports, instance_id)?;
        store.link_group = Some(group.clone());
        let store = alloc::rc::Rc::new(core::cell::RefCell::new(store));
        // Register the instance even when instantiation failed partway:
        // spec semantics persist element segment writes into shared tables
        // from failed instantiations, and those functions remain callable.
        group
            .borrow_mut()
            .insert(instance_id, (module.clone(), store.clone()));
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
        let (store, error) = Self::instantiate_internal(module, imports, 0)?;
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
                    min: (shared.borrow().len() / PAGE_SIZE) as u32,
                    max: provider.ty.limits.max,
                },
                None => provider.ty.limits,
            };
            if !limits_match(&provided_limits, &declared.ty.limits) {
                return Err(import_type_mismatch(&declared.module, &declared.name));
            }
            let entity = match provider.shared.clone() {
                Some(shared) => shared,
                None => {
                    let bytes = try_alloc_n(provider.ty.limits.min as usize * PAGE_SIZE, 0u8)
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
            if !global_type_matches(&provider.ty, &declared.ty) {
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
                    min: shared.borrow().len() as u32,
                    max: provider.ty.limits.max,
                },
                None => provider.ty.limits,
            };
            if provider.ty.elem != declared.ty.elem
                || !limits_match(&provided_limits, &declared.ty.limits)
            {
                return Err(import_type_mismatch(&declared.module, &declared.name));
            }
            let entity = match provider.shared.clone() {
                Some(shared) => shared,
                None => {
                    let values = try_alloc_n(
                        provider.ty.limits.min as usize,
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
        let mut globals_plain: Vec<Value> = Vec::with_capacity(module.globals.len());
        let mut globals: Vec<alloc::rc::Rc<core::cell::Cell<Value>>> =
            Vec::with_capacity(module.globals.len());
        let mut global_types = Vec::with_capacity(module.globals.len());
        for global in &module.globals {
            let value = eval_const(
                &global.init,
                &globals_plain,
                &imported_globals_plain,
                module.imported_global_count,
                instance_id,
            )?;
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
            let bytes = try_alloc_n(mem.limits.min as usize * PAGE_SIZE, 0u8)
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
                    eval_const(
                        &lowered,
                        &globals_plain,
                        &imported_globals_plain,
                        module.imported_global_count,
                        instance_id,
                    )?
                }
                None => table_null(table.clone()),
            };
            let values = try_alloc_n(table.limits.min as usize, fill)
                .ok_or_else(|| allocation_failed("table"))?;
            tables.push(alloc::rc::Rc::new(core::cell::RefCell::new(values)));
        }
        let table_types = module.tables.clone();

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
        };

        let mut instantiation_error: Option<RuntimeError> = None;

        // Element segments: active ones are written into their tables (and
        // dropped); passive ones are retained for `table.init`. On the first
        // failure, remaining segments are skipped but earlier writes persist
        // (spec: writes before the failure persist in shared tables).
        for (idx, segment) in module.elements.iter().enumerate() {
            if instantiation_error.is_some() {
                break;
            }
            let values: Vec<Value> = segment
                .values
                .iter()
                .map(|value| match *value {
                    RegElemValue::FuncRef(func) => Value::FuncRef(Some((instance_id, func.0))),
                    RegElemValue::GlobalGet(global) => {
                        let idx = global.0 as usize;
                        if idx < store.imported_global_count as usize {
                            store
                                .imported_global_values
                                .get(idx)
                                .expect("imported globals resolved")
                                .get()
                        } else {
                            store
                                .globals
                                .get(idx - store.imported_global_count as usize)
                                .expect("defined globals initialized")
                                .get()
                        }
                    }
                    RegElemValue::Null => Value::FuncRef(None),
                })
                .collect();
            match &segment.mode {
                RegElementMode::Active { table, offset } => {
                    let offset = match eval_const(
                        offset,
                        &globals_plain,
                        &imported_globals_plain,
                        store.imported_global_count,
                        store.instance_id,
                    ) {
                        Ok(offset) => offset,
                        Err(error) => {
                            instantiation_error = Some(error);
                            break;
                        }
                    };
                    let Value::I32(offset) = offset else {
                        instantiation_error = Some(RuntimeError {
                            kind: RuntimeErrorKind::InvalidConstExpr,
                        });
                        break;
                    };
                    let applied = store
                        .with_table_mut(table.0, |target| {
                            let start = offset as usize;
                            let Some(end) = start.checked_add(values.len()) else {
                                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
                            };
                            if end > target.len() {
                                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
                            }
                            target[start..end].copy_from_slice(&values);
                            Ok(())
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
                    let offset = match eval_const(
                        offset,
                        &globals_plain,
                        &imported_globals_plain,
                        store.imported_global_count,
                        store.instance_id,
                    ) {
                        Ok(offset) => offset,
                        Err(error) => {
                            instantiation_error = Some(error);
                            break;
                        }
                    };
                    let Value::I32(offset) = offset else {
                        instantiation_error = Some(RuntimeError {
                            kind: RuntimeErrorKind::InvalidConstExpr,
                        });
                        break;
                    };
                    let applied = store
                        .with_memory_mut(memory.0, |mem| {
                            let start = offset as usize;
                            let Some(end) = start.checked_add(segment.bytes.len()) else {
                                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                            };
                            if end > mem.len() {
                                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                            }
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
        crate::runtime::execute_func_in(Some(module), Some(self), func, &[], 0)?;
        Ok(())
    }

    /// The shared handle of a memory by index-space index (imported first).
    pub(crate) fn shared_memory(
        &self,
        idx: u32,
    ) -> Option<alloc::rc::Rc<core::cell::RefCell<Vec<u8>>>> {
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
    pub(crate) fn with_table<R>(&self, idx: u32, f: impl FnOnce(&[Value]) -> R) -> Option<R> {
        let shared = self.shared_table(idx)?;
        let table = shared.borrow();
        Some(f(&table))
    }

    /// Mutate a table by index-space index (imported first) via closure.
    pub(crate) fn with_table_mut<R>(
        &self,
        idx: u32,
        f: impl FnOnce(&mut [Value]) -> R,
    ) -> Option<R> {
        let shared = self.shared_table(idx)?;
        let mut table = shared.borrow_mut();
        Some(f(&mut table))
    }

    /// The shared handle of a table by index-space index (imported first).
    pub(crate) fn shared_table(
        &self,
        idx: u32,
    ) -> Option<alloc::rc::Rc<core::cell::RefCell<Vec<Value>>>> {
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
    ) -> Option<(TableType, alloc::rc::Rc<core::cell::RefCell<Vec<Value>>>)> {
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
        let Some(pos) = self
            .imported_funcs
            .iter()
            .position(|import| import.module == module && import.name == name)
        else {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownImport {
                    module: module.into(),
                    name: name.into(),
                },
            });
        };
        if func.ty() != &self.imported_funcs[pos].ty {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::ImportTypeMismatch {
                    module: module.into(),
                    name: name.into(),
                },
            });
        }
        self.host_funcs[pos] = Some(core::cell::RefCell::new(func));
        Ok(())
    }

    /// This instance's id within its link group (0 when unlinked).
    pub(crate) fn instance_id(&self) -> u32 {
        self.instance_id
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
        self.host_funcs[func_idx as usize]
            .as_ref()
            .expect("registration checked above")
            .borrow_mut()
            .call(args)
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
                    .with_gpu_mut(|gpu| gpu.compile("vadd", WGSL_F32_ADD))
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
        self.with_gpu_mut(|gpu| gpu.dispatch(kernel, &[buf_a, buf_b, buf_out], workgroups))
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

/// Evaluate a lowered const expression against the globals initialized so
/// far. Used for global initializers and data segment offsets.
fn eval_const(
    expr: &[RegConstInstr],
    globals: &[Value],
    imported_globals: &[Value],
    imported_global_count: u32,
    instance_id: u32,
) -> Result<Value, RuntimeError> {
    let mut stack: Vec<Value> = Vec::new();
    for instr in expr {
        match *instr {
            RegConstInstr::I32Const(value) => stack.push(Value::I32(value)),
            RegConstInstr::I64Const(value) => stack.push(Value::I64(value)),
            RegConstInstr::F32Const(bits) => stack.push(Value::F32(f32::from_bits(bits))),
            RegConstInstr::F64Const(bits) => stack.push(Value::F64(f64::from_bits(bits))),
            RegConstInstr::RefNull => stack.push(Value::FuncRef(None)),
            RegConstInstr::RefFunc(func) => {
                stack.push(Value::FuncRef(Some((instance_id, func.0))));
            }
            RegConstInstr::GlobalGet(global) => {
                let idx = global.0 as usize;
                let value = if idx < imported_global_count as usize {
                    imported_globals.get(idx).copied().ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
                    })?
                } else {
                    globals
                        .get(idx - imported_global_count as usize)
                        .copied()
                        .ok_or(RuntimeError {
                            kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
                        })?
                };
                stack.push(value);
            }
            RegConstInstr::I32Add => {
                let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                stack.push(Value::I32(lhs.wrapping_add(rhs)));
            }
            RegConstInstr::I32Sub => {
                let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                stack.push(Value::I32(lhs.wrapping_sub(rhs)));
            }
            RegConstInstr::I32Mul => {
                let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                stack.push(Value::I32(lhs.wrapping_mul(rhs)));
            }
            RegConstInstr::I64Add => {
                let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                stack.push(Value::I64(lhs.wrapping_add(rhs)));
            }
            RegConstInstr::I64Sub => {
                let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                stack.push(Value::I64(lhs.wrapping_sub(rhs)));
            }
            RegConstInstr::I64Mul => {
                let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                stack.push(Value::I64(lhs.wrapping_mul(rhs)));
            }
        }
    }

    if stack.len() != 1 {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    }
    Ok(stack[0])
}

fn pop_i32_pair(stack: &mut Vec<Value>) -> Result<(i32, i32), RuntimeError> {
    let (Some(Value::I32(rhs)), Some(Value::I32(lhs))) = (stack.pop(), stack.pop()) else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    };
    Ok((lhs, rhs))
}

fn pop_i64_pair(stack: &mut Vec<Value>) -> Result<(i64, i64), RuntimeError> {
    let (Some(Value::I64(rhs)), Some(Value::I64(lhs))) = (stack.pop(), stack.pop()) else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    };
    Ok((lhs, rhs))
}
