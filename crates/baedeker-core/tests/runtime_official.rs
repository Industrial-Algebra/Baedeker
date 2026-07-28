//! Official WebAssembly spec test suite (`spec/wast-official`) execution
//! harness.
//!
//! Extends the curated runtime harness with the machinery the official
//! suite needs: named module instances with `(register)`-based linking, the
//! standard `spectest` import shim, and malformed/invalid/unlinkable
//! directive handling. Files whose required features Baedeker does not yet
//! implement are deferred with a recorded reason (see `DEFERRED_FILES`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    HostFunction, Imports, RuntimeError, RuntimeErrorKind, Store, Value, execute_export, link_func,
};
use baedeker_core::types::{
    FuncType, GlobalType, MemType, Mutability, NumType, TableType, ValType,
};
use wast::core::{WastArgCore, WastRetCore};
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastRet};

/// Files deferred with a reason (feature not yet implemented). Every entry
/// here is a known gap; keep the list honest and shrinking.
const DEFERRED_FILES: &[(&str, &str)] = &[
    // Tail calls: return_call/return_call_indirect not lowered.
    ("return_call.wast", "tail calls not implemented"),
    ("return_call_indirect.wast", "tail calls not implemented"),
    ("return_call_ref.wast", "tail calls not implemented"),
    // Function-references / GC-era references.
    ("ref_null.wast", "GC heap types (anyref) not implemented"),
    (
        "type-canon.wast",
        "GC type canonicalization not implemented",
    ),
    (
        "type-equivalence.wast",
        "GC type equivalence not implemented",
    ),
    ("type-rec.wast", "recursive types not implemented"),
    ("type.wast", "recursive/GC types not implemented"),
    ("exceptions", "exception handling not implemented"),
    // Guard-page semantics are host/OS-specific.
    (
        "skip-stack-guard-page.wast",
        "stack guard pages not applicable",
    ),
    // Exception handling (tag section).
    ("instance.wast", "tag section (exceptions) not implemented"),
    ("imports.wast", "tag section (exceptions) not implemented"),
    // The wast parser rejects the confusable unicode names.wast needs.
    ("names.wast", "parser rejects confusable unicode in names"),
    // Full SIMD surface beyond the current core subset.
    ("simd", "full SIMD surface not implemented"),
];

#[derive(Debug, Default)]
struct OfficialStats {
    files: usize,
    deferred: usize,
    directives: usize,
    modules: usize,
    assertions: usize,
    failures: Vec<String>,
}

struct Instance {
    module: Rc<RegModule>,
    store: Rc<RefCell<Store>>,
}

struct Registry {
    /// Instances by explicit name (`$id`) and by registered string.
    named: BTreeMap<String, Instance>,
    last: Option<Instance>,
    /// The link group all instances in this file share.
    group: Rc<RefCell<baedeker_core::runtime::LinkGroup>>,
    next_instance_id: u32,
}

impl Registry {
    fn new() -> Self {
        Self {
            named: BTreeMap::new(),
            last: None,
            group: Rc::new(RefCell::new(BTreeMap::new())),
            next_instance_id: 1,
        }
    }

    fn fresh_instance_id(&mut self) -> u32 {
        let id = self.next_instance_id;
        self.next_instance_id += 1;
        id
    }

    fn insert(&mut self, key: Option<&str>, instance: Instance) {
        if let Some(key) = key {
            self.named.insert(key.to_owned(), instance.clone());
        }
        self.last = Some(instance);
    }

    fn register(&mut self, name: &str, id: Option<&str>) {
        let instance = match id {
            Some(id) => self.named.get(id).cloned(),
            None => self.last.clone(),
        };
        if let Some(instance) = instance {
            self.named.insert(name.to_owned(), instance);
        }
    }

    fn get(&self, id: Option<&str>) -> Option<&Instance> {
        match id {
            Some(id) => self.named.get(id),
            None => self.last.as_ref(),
        }
    }
}

impl Clone for Instance {
    fn clone(&self) -> Self {
        Self {
            module: self.module.clone(),
            store: self.store.clone(),
        }
    }
}

fn main_panic(path: &Path, msg: &str) -> ! {
    panic!("{}: {msg}", path.display());
}

/// Format a runtime error with spec trap messages where applicable.
fn format_runtime_error(error: &RuntimeError) -> String {
    match error.kind {
        RuntimeErrorKind::Trap(trap) => trap.wast_message().to_owned(),
        _ => format!("{error:?}"),
    }
}

/// Instantiate a lowered module, resolving imports from `spectest` and
/// registered instances.
fn instantiate(
    _path: &Path,
    registry: &mut Registry,
    module: RegModule,
) -> Result<Instance, String> {
    let module = Rc::new(module);

    // State imports: resolve eagerly against spectest + linked instances.
    let mut imports = Imports::new();
    for declared in &module.imported_memories {
        imports = if declared.module == "spectest" {
            imports.memory("spectest", &declared.name, spectest_memory(&declared.name)?)
        } else {
            let (ty, shared) = linked_memory(registry, &declared.module, &declared.name)?;
            imports.shared_memory(&declared.module, &declared.name, ty, shared)
        };
    }
    for declared in &module.imported_globals {
        imports = if declared.module == "spectest" {
            let (ty, value) = spectest_global(&declared.name)?;
            imports.global("spectest", &declared.name, ty, value)
        } else {
            let (ty, shared) = linked_global(registry, &declared.module, &declared.name)?;
            imports.shared_global(&declared.module, &declared.name, ty, shared)
        };
    }
    for declared in &module.imported_tables {
        imports = if declared.module == "spectest" {
            imports.table("spectest", &declared.name, spectest_table(&declared.name)?)
        } else {
            let (ty, shared) = linked_table(registry, &declared.module, &declared.name)?;
            imports.shared_table(&declared.module, &declared.name, ty, shared)
        };
    }

    let instance_id = registry.fresh_instance_id();
    let store_rc = Store::instantiate_linked(&module, &imports, &registry.group, instance_id)
        .map_err(|error| format_runtime_error(&error))?;
    let mut store_guard = store_rc.borrow_mut();
    let store = &mut *store_guard;

    // Function imports: link lazily against spectest + registered instances.
    for declared in &module.imported_funcs {
        let host = if declared.module == "spectest" {
            spectest_host_func(&declared.name)?
        } else {
            let source = registry.get(Some(&declared.module)).ok_or_else(|| {
                format!(
                    "link: module {:?} not registered for import {:?}",
                    declared.module, declared.name
                )
            })?;
            let func_idx = source
                .store
                .borrow()
                .export_func(&declared.name)
                .ok_or_else(|| {
                    format!(
                        "link: no function export {:?} on {:?}",
                        declared.name, declared.module
                    )
                })?;
            // Type-check the linked function against the import declaration
            // (spec: mismatched signatures are a link error).
            let source_ty = &source
                .module
                .funcs
                .iter()
                .find(|func| func.idx == func_idx)
                .map(|func| &source.module.types[func.type_idx.0 as usize]);
            match source_ty {
                Some(source_ty) if **source_ty == declared.ty => {}
                Some(source_ty) => {
                    return Err(format!(
                        "link: type mismatch on {:?}.{:?}: source {source_ty:?} vs import {:?}",
                        declared.module, declared.name, declared.ty
                    ));
                }
                None => {
                    return Err(format!(
                        "link: function {func_idx:?} not found in {:?}",
                        declared.module
                    ));
                }
            }
            link_func(
                source.module.clone(),
                source.store.clone(),
                func_idx,
                declared.ty.clone(),
            )
        };
        store
            .register_host_func(&declared.module, &declared.name, host)
            .map_err(|error| format!("register host func failed: {error:?}"))?;
    }

    // Run the deferred start function now that host functions are registered.
    if let Err(error) = store.run_start(&module) {
        return Err(format_runtime_error(&error));
    }

    drop(store_guard);
    Ok(Instance {
        module: module.clone(),
        store: store_rc,
    })
}

type SharedMemory = (MemType, Rc<RefCell<Vec<u8>>>);
type SharedTable = (TableType, Rc<RefCell<baedeker_core::runtime::Table>>);

fn linked_memory(registry: &Registry, module: &str, name: &str) -> Result<SharedMemory, String> {
    let source = registry
        .get(Some(module))
        .ok_or_else(|| format!("link: module {module:?} not registered"))?;
    source
        .store
        .borrow()
        .export_memory(name)
        .ok_or_else(|| format!("link: no memory export {name:?} on {module:?}"))
}

fn linked_global(
    registry: &Registry,
    module: &str,
    name: &str,
) -> Result<(GlobalType, Rc<core::cell::Cell<Value>>), String> {
    let source = registry
        .get(Some(module))
        .ok_or_else(|| format!("link: module {module:?} not registered"))?;
    let found = source
        .store
        .borrow()
        .export_global(name)
        .ok_or_else(|| format!("link: no global export {name:?} on {module:?}"))?;
    Ok(found)
}

fn linked_table(registry: &Registry, module: &str, name: &str) -> Result<SharedTable, String> {
    let source = registry
        .get(Some(module))
        .ok_or_else(|| format!("link: module {module:?} not registered"))?;
    source
        .store
        .borrow()
        .export_table(name)
        .ok_or_else(|| format!("link: no table export {name:?} on {module:?}"))
}

// ── spectest shim ─────────────────────────────────────────────────

fn spectest_memory(name: &str) -> Result<MemType, String> {
    match name {
        "memory" => Ok(MemType {
            limits: baedeker_core::types::Limits {
                min: 1,
                max: Some(2),
            },
        }),
        other => Err(format!("unknown spectest memory {other:?}")),
    }
}

fn spectest_global(name: &str) -> Result<(GlobalType, Value), String> {
    let (ty, value) = match name {
        "global_i32" => (ValType::Num(NumType::I32), Value::I32(666)),
        "global_i64" => (ValType::Num(NumType::I64), Value::I64(666)),
        "global_f32" => (ValType::Num(NumType::F32), Value::F32(666.6)),
        "global_f64" => (ValType::Num(NumType::F64), Value::F64(666.6)),
        other => return Err(format!("unknown spectest global {other:?}")),
    };
    Ok((
        GlobalType {
            val_type: ty,
            mutability: Mutability::Const,
        },
        value,
    ))
}

fn spectest_table(name: &str) -> Result<TableType, String> {
    match name {
        "table" => Ok(TableType {
            elem: baedeker_core::types::RefType::FuncRef,
            limits: baedeker_core::types::Limits {
                min: 10,
                max: Some(20),
            },
            init: None,
        }),
        other => Err(format!("unknown spectest table {other:?}")),
    }
}

fn spectest_host_func(name: &str) -> Result<HostFunction, String> {
    let (params, results) = match name {
        "print" => (vec![], vec![]),
        "print_i32" => (vec![ValType::Num(NumType::I32)], vec![]),
        "print_i64" => (vec![ValType::Num(NumType::I64)], vec![]),
        "print_f32" => (vec![ValType::Num(NumType::F32)], vec![]),
        "print_f64" => (vec![ValType::Num(NumType::F64)], vec![]),
        "print_i32_f32" => (
            vec![ValType::Num(NumType::I32), ValType::Num(NumType::F32)],
            vec![],
        ),
        "print_f64_f64" => (
            vec![ValType::Num(NumType::F64), ValType::Num(NumType::F64)],
            vec![],
        ),
        other => return Err(format!("unknown spectest function {other:?}")),
    };
    Ok(HostFunction::new(FuncType { params, results }, |_args| {
        Ok(vec![])
    }))
}

// ── directive execution ─────────────────────────────────────────────

fn run_file(path: &Path, stats: &mut OfficialStats) {
    let text = baedeker_testdata::spec_case_text(path);
    let buf = match ParseBuffer::new(&text) {
        Ok(buf) => buf,
        Err(error) => main_panic(path, &format!("failed to parse wast buffer: {error}")),
    };
    let wast = match parse::<Wast<'_>>(&buf) {
        Ok(wast) => wast,
        Err(error) => main_panic(path, &format!("failed to parse wast directives: {error}")),
    };

    let mut registry = Registry::new();

    for directive in wast.directives {
        stats.directives += 1;
        match directive {
            WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat) => {
                stats.modules += 1;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let (name, module) = lower_wat(path, wat);
                    instantiate(path, &mut registry, module).map(|instance| (name, instance))
                }));
                match outcome {
                    Ok(Ok((name, instance))) => registry.insert(name.as_deref(), instance),
                    Ok(Err(msg)) => stats.failures.push(format!("{}: {msg}", path.display())),
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                            .unwrap_or_else(|| "unknown panic".to_owned());
                        stats
                            .failures
                            .push(format!("{}: panicked: {msg}", path.display()));
                    }
                }
            }
            WastDirective::AssertReturn { exec, results, .. } => {
                stats.assertions += 1;
                assert_return(path, &registry, exec, results, stats);
            }
            WastDirective::AssertTrap { exec, message, .. } => {
                stats.assertions += 1;
                match exec {
                    WastExecute::Wat(module) => {
                        // Instantiation-time trap (e.g. out-of-bounds active
                        // data segment).
                        let (name, lowered) = lower_wat(path, QuoteWat::Wat(module));
                        let _ = name;
                        match instantiate(path, &mut registry, lowered) {
                            Err(msg) => {
                                if !msg.contains(message) {
                                    stats.failures.push(format!(
                                        "{}: assert_trap expected {message:?}, got {msg:?}",
                                        path.display()
                                    ));
                                }
                            }
                            Ok(_) => stats.failures.push(format!(
                                "{}: assert_trap expected {message:?} but instantiation succeeded",
                                path.display()
                            )),
                        }
                    }
                    _ => assert_trap(path, &registry, exec, message, stats),
                }
            }
            WastDirective::AssertExhaustion { call, message, .. } => {
                stats.assertions += 1;
                assert_exhaustion(path, &registry, call, message, stats);
            }
            WastDirective::AssertMalformed { module, .. } => {
                stats.assertions += 1;
                // If the module text encodes at all, it must be rejected at
                // some stage: binary structure, instruction bodies, or
                // validation. (Instruction decoding is lazy, so body-level
                // malformation surfaces during lowering.)
                if let QuoteWat::Wat(mut wat) = module
                    && let Ok(bytes) = wat.encode()
                {
                    let accepted = Module::decode(&bytes)
                        .map_err(|_| ())
                        .and_then(|module| module.lower().map(|_| ()).map_err(|_| ()))
                        .is_ok();
                    if accepted {
                        stats.failures.push(format!(
                            "{}: assert_malformed module accepted",
                            path.display()
                        ));
                    }
                }
            }
            WastDirective::AssertInvalid { module, span, .. } => {
                stats.assertions += 1;
                let bytes = encode_wat_directive(path, module, stats);
                if let Some(bytes) = bytes
                    && Module::decode(&bytes)
                        .map_err(|error| error.to_string())
                        .and_then(|module| module.lower().map_err(|error| format!("{error:?}")))
                        .is_ok()
                {
                    stats.failures.push(format!(
                        "{}: assert_invalid module at offset {} lowered successfully",
                        path.display(),
                        span.offset()
                    ));
                }
            }
            WastDirective::AssertUnlinkable { module, span, .. } => {
                stats.assertions += 1;
                let (name, lowered) = lower_wat(path, QuoteWat::Wat(module));
                let _ = name;
                if instantiate(path, &mut registry, lowered).is_ok() {
                    stats.failures.push(format!(
                        "{}: assert_unlinkable module at offset {} instantiated successfully",
                        path.display(),
                        span.offset()
                    ));
                }
            }
            WastDirective::Register { name, module, .. } => {
                registry.register(name, module.map(|id| id.name()));
            }
            WastDirective::Invoke(invoke) => {
                stats.assertions += 1;
                execute_invoke(
                    path,
                    &registry,
                    invoke.module.map(|id| id.name()),
                    invoke.name,
                    &invoke_args(&invoke.args),
                    stats,
                );
            }
            other => stats.failures.push(format!(
                "{}: unsupported directive {other:?}",
                path.display()
            )),
        }
    }
}

fn encode_wat_directive(
    path: &Path,
    module: QuoteWat<'_>,
    stats: &mut OfficialStats,
) -> Option<Vec<u8>> {
    let mut module = module;
    match module.encode() {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            stats.failures.push(format!(
                "{}: failed to encode module: {error}",
                path.display()
            ));
            None
        }
    }
}

fn lower_wat(path: &Path, wat: QuoteWat<'_>) -> (Option<String>, RegModule) {
    let (name, bytes) = match wat {
        QuoteWat::Wat(wast::Wat::Module(module)) => {
            let name = module.id.map(|id| id.name().to_owned());
            let mut wat = wast::Wat::Module(module);
            let bytes = wat.encode().unwrap_or_else(|error| {
                main_panic(path, &format!("failed to encode module: {error}"))
            });
            (name, bytes)
        }
        QuoteWat::Wat(mut wat) => {
            let bytes = wat.encode().unwrap_or_else(|error| {
                main_panic(path, &format!("failed to encode module: {error}"))
            });
            (None, bytes)
        }
        QuoteWat::QuoteModule(_, chunks) => {
            // Raw text module: join the chunks and re-parse as a module.
            let text: String = chunks
                .iter()
                .map(|(_, chunk)| String::from_utf8_lossy(chunk).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            let bytes = (|| {
                let buf = ParseBuffer::new(&text).ok()?;
                let mut wat = parse::<wast::Wat<'_>>(&buf).ok()?;
                wat.encode().ok()
            })()
            .unwrap_or_else(|| main_panic(path, "quote module re-parse/encode failed"));
            (None, bytes)
        }
        _ => main_panic(path, "expected a module directive"),
    };
    let module = Module::decode(&bytes)
        .unwrap_or_else(|error| main_panic(path, &format!("expected module to decode: {error}")));
    let lowered = module
        .lower()
        .unwrap_or_else(|error| main_panic(path, &format!("expected module to lower: {error:?}")));
    (name, lowered)
}

fn invoke_args(args: &[WastArg<'_>]) -> Vec<Value> {
    args.iter()
        .map(|arg| match arg {
            WastArg::Core(WastArgCore::I32(value)) => Value::I32(*value),
            WastArg::Core(WastArgCore::I64(value)) => Value::I64(*value),
            WastArg::Core(WastArgCore::F32(value)) => Value::F32(f32::from_bits(value.bits)),
            WastArg::Core(WastArgCore::F64(value)) => Value::F64(f64::from_bits(value.bits)),
            WastArg::Core(WastArgCore::RefNull(ty)) => match ty {
                wast::core::HeapType::Abstract {
                    ty: wast::core::AbstractHeapType::Extern,
                    ..
                } => Value::ExternRef(None),
                _ => Value::FuncRef(None),
            },
            WastArg::Core(WastArgCore::RefExtern(value))
            | WastArg::Core(WastArgCore::RefHost(value)) => Value::ExternRef(Some(*value)),
            other => panic!("unsupported invoke argument: {other:?}"),
        })
        .collect()
}

fn execute_invoke(
    path: &Path,
    registry: &Registry,
    module_id: Option<&str>,
    name: &str,
    args: &[Value],
    stats: &mut OfficialStats,
) -> Option<Result<Vec<Value>, RuntimeError>> {
    let Some(instance) = registry.get(module_id) else {
        stats.failures.push(format!(
            "{}: no instance for invoke {name:?}",
            path.display()
        ));
        return None;
    };
    let store = instance.store.borrow();
    Some(execute_export(&instance.module, &store, name, args))
}

fn assert_return(
    path: &Path,
    registry: &Registry,
    exec: WastExecute<'_>,
    expected: Vec<WastRet<'_>>,
    stats: &mut OfficialStats,
) {
    let WastExecute::Invoke(invoke) = exec else {
        if let WastExecute::Get { module, global, .. } = exec {
            let Some(instance) = registry.get(module.map(|id| id.name())) else {
                stats.failures.push(format!(
                    "{}: no instance for get {global:?}",
                    path.display()
                ));
                return;
            };
            let store = instance.store.borrow();
            let Some((_, cell)) = store.export_global(global) else {
                stats
                    .failures
                    .push(format!("{}: no exported global {global:?}", path.display()));
                return;
            };
            let actual = cell.get();
            if expected.len() == 1 && result_matches(&actual, &expected[0]) {
                return;
            }
            stats.failures.push(format!(
                "{}: assert_return (get {global:?}) mismatch: actual {actual:?}, expected {expected:?}",
                path.display()
            ));
            return;
        }
        stats.failures.push(format!(
            "{}: non-invoke assert_return unsupported",
            path.display()
        ));
        return;
    };
    let args = invoke_args(&invoke.args);
    let Some(result) = execute_invoke(
        path,
        registry,
        invoke.module.map(|id| id.name()),
        invoke.name,
        &args,
        stats,
    ) else {
        return;
    };
    match result {
        Ok(actual) => {
            if actual.len() != expected.len()
                || !actual
                    .iter()
                    .zip(expected.iter())
                    .all(|(actual, expected)| result_matches(actual, expected))
            {
                stats.failures.push(format!(
                    "{}: assert_return mismatch on {:?}: actual {actual:?}, expected {expected:?}",
                    path.display(),
                    invoke.name
                ));
            }
        }
        Err(error) => stats.failures.push(format!(
            "{}: assert_return invoke {:?} trapped: {error:?}",
            path.display(),
            invoke.name
        )),
    }
}

fn assert_trap(
    path: &Path,
    registry: &Registry,
    exec: WastExecute<'_>,
    message: &str,
    stats: &mut OfficialStats,
) {
    let WastExecute::Invoke(invoke) = exec else {
        stats.failures.push(format!(
            "{}: non-invoke assert_trap unsupported",
            path.display()
        ));
        return;
    };
    let args = invoke_args(&invoke.args);
    let Some(result) = execute_invoke(
        path,
        registry,
        invoke.module.map(|id| id.name()),
        invoke.name,
        &args,
        stats,
    ) else {
        return;
    };
    match result {
        Err(error) => {
            let RuntimeErrorKind::Trap(trap) = error.kind else {
                stats.failures.push(format!(
                    "{}: assert_trap expected {message:?}, got non-trap error: {error:?}",
                    path.display()
                ));
                return;
            };
            if trap.wast_message() != message {
                stats.failures.push(format!(
                    "{}: assert_trap message mismatch: got {:?}, expected {message:?}",
                    path.display(),
                    trap.wast_message()
                ));
            }
        }
        Ok(values) => stats.failures.push(format!(
            "{}: assert_trap expected {message:?} but invoke {:?} returned {values:?}",
            path.display(),
            invoke.name
        )),
    }
}

fn assert_exhaustion(
    path: &Path,
    registry: &Registry,
    invoke: wast::WastInvoke<'_>,
    message: &str,
    stats: &mut OfficialStats,
) {
    let args = invoke_args(&invoke.args);
    let Some(result) = execute_invoke(
        path,
        registry,
        invoke.module.map(|id| id.name()),
        invoke.name,
        &args,
        stats,
    ) else {
        return;
    };
    match result {
        Err(error) => {
            let RuntimeErrorKind::Trap(trap) = error.kind else {
                stats.failures.push(format!(
                    "{}: assert_exhaustion expected {message:?}, got non-trap error: {error:?}",
                    path.display()
                ));
                return;
            };
            if trap.wast_message() != message {
                stats.failures.push(format!(
                    "{}: assert_exhaustion message mismatch: got {:?}, expected {message:?}",
                    path.display(),
                    trap.wast_message()
                ));
            }
        }
        Ok(values) => stats.failures.push(format!(
            "{}: assert_exhaustion expected {message:?} but invoke returned {values:?}",
            path.display()
        )),
    }
}

/// Whether an actual runtime value satisfies an expected WAST result,
/// including NaN patterns and v128 lane patterns.
fn result_matches(actual: &Value, expected: &WastRet<'_>) -> bool {
    match expected {
        WastRet::Core(WastRetCore::I32(value)) => *actual == Value::I32(*value),
        WastRet::Core(WastRetCore::I64(value)) => *actual == Value::I64(*value),
        WastRet::Core(WastRetCore::F32(pattern)) => match (actual, pattern) {
            (Value::F32(actual), wast::core::NanPattern::Value(expected)) => {
                actual.to_bits() == expected.bits
            }
            (Value::F32(actual), wast::core::NanPattern::CanonicalNan) => {
                actual.to_bits() & 0x7fff_ffff == 0x7fc0_0000
            }
            (Value::F32(actual), wast::core::NanPattern::ArithmeticNan) => actual.is_nan(),
            _ => false,
        },
        WastRet::Core(WastRetCore::F64(pattern)) => match (actual, pattern) {
            (Value::F64(actual), wast::core::NanPattern::Value(expected)) => {
                actual.to_bits() == expected.bits
            }
            (Value::F64(actual), wast::core::NanPattern::CanonicalNan) => {
                actual.to_bits() & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000
            }
            (Value::F64(actual), wast::core::NanPattern::ArithmeticNan) => actual.is_nan(),
            _ => false,
        },
        WastRet::Core(WastRetCore::RefNull(_)) => {
            matches!(actual, Value::FuncRef(None) | Value::ExternRef(None))
        }
        WastRet::Core(WastRetCore::RefExtern(Some(expected))) => {
            *actual == Value::ExternRef(Some(*expected))
        }
        WastRet::Core(WastRetCore::RefExtern(None)) => actual == &Value::ExternRef(None),
        // `(ref.func)` with no index: any non-null funcref.
        WastRet::Core(WastRetCore::RefFunc(None)) => matches!(actual, Value::FuncRef(Some(_))),
        WastRet::Core(WastRetCore::RefFunc(Some(expected))) => {
            let expected_idx = match expected {
                wast::token::Index::Num(idx, _) => Some(*idx),
                wast::token::Index::Id(_) => None,
            };
            match (actual, expected_idx) {
                (Value::FuncRef(Some((_, actual_idx))), Some(expected_idx)) => {
                    actual_idx == &expected_idx
                }
                (Value::FuncRef(Some(_)), None) => true,
                _ => false,
            }
        }
        WastRet::Core(WastRetCore::RefHost(_)) => false,
        WastRet::Core(WastRetCore::V128(pattern)) => {
            let Value::V128(actual) = actual else {
                return false;
            };
            v128_pattern_matches(actual, pattern)
        }
        _ => false,
    }
}

fn v128_pattern_matches(actual: &[u8; 16], pattern: &wast::core::V128Pattern) -> bool {
    match pattern {
        wast::core::V128Pattern::I8x16(lanes) => actual
            .iter()
            .zip(lanes.iter())
            .all(|(actual, expected)| *actual == *expected as u8),
        wast::core::V128Pattern::I16x8(lanes) => lanes
            .iter()
            .enumerate()
            .all(|(i, expected)| actual[i * 2..i * 2 + 2] == expected.to_le_bytes()),
        wast::core::V128Pattern::I32x4(lanes) => lanes
            .iter()
            .enumerate()
            .all(|(i, expected)| actual[i * 4..i * 4 + 4] == expected.to_le_bytes()),
        wast::core::V128Pattern::I64x2(lanes) => lanes
            .iter()
            .enumerate()
            .all(|(i, expected)| actual[i * 8..i * 8 + 8] == expected.to_le_bytes()),
        wast::core::V128Pattern::F32x4(lanes) => lanes.iter().enumerate().all(|(i, pattern)| {
            let bits = u32::from_le_bytes(actual[i * 4..i * 4 + 4].try_into().expect("lane width"));
            nan_pattern_matches_f32(f32::from_bits(bits), pattern)
        }),
        wast::core::V128Pattern::F64x2(lanes) => lanes.iter().enumerate().all(|(i, pattern)| {
            let bits = u64::from_le_bytes(actual[i * 8..i * 8 + 8].try_into().expect("lane width"));
            nan_pattern_matches_f64(f64::from_bits(bits), pattern)
        }),
    }
}

fn nan_pattern_matches_f32(
    actual: f32,
    pattern: &wast::core::NanPattern<wast::token::F32>,
) -> bool {
    match pattern {
        wast::core::NanPattern::Value(expected) => actual.to_bits() == expected.bits,
        wast::core::NanPattern::CanonicalNan => actual.to_bits() & 0x7fff_ffff == 0x7fc0_0000,
        wast::core::NanPattern::ArithmeticNan => actual.is_nan(),
    }
}

fn nan_pattern_matches_f64(
    actual: f64,
    pattern: &wast::core::NanPattern<wast::token::F64>,
) -> bool {
    match pattern {
        wast::core::NanPattern::Value(expected) => actual.to_bits() == expected.bits,
        wast::core::NanPattern::CanonicalNan => {
            actual.to_bits() & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000
        }
        wast::core::NanPattern::ArithmeticNan => actual.is_nan(),
    }
}

fn is_deferred(path: &Path) -> Option<&'static str> {
    let file = path.file_name()?.to_str()?;
    DEFERRED_FILES
        .iter()
        .find(|(name, _)| *name == file || file.starts_with(name))
        .map(|(_, reason)| *reason)
}

#[test]
fn official_spec_cases() {
    // Run on a thread with a large stack so deep (bounded) recursion in the
    // spec suite fits within MAX_CALL_DEPTH and the host stack.
    let handle = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(official_spec_cases_inner)
        .expect("spawn official spec thread");
    handle.join().expect("official spec thread panicked");
}

fn official_spec_cases_inner() {
    let mut stats = OfficialStats::default();
    let mut deferred: Vec<(PathBuf, &'static str)> = Vec::new();

    for path in baedeker_testdata::spec_wast_cases("wast-official") {
        if let Ok(filter) = std::env::var("BAEDEKER_OFFICIAL_FILTER")
            && !filter.is_empty()
            && !path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .contains(&filter)
        {
            continue;
        }
        if let Some(reason) = is_deferred(&path) {
            stats.deferred += 1;
            deferred.push((path, reason));
            continue;
        }
        stats.files += 1;
        let mut file_stats = OfficialStats {
            files: 1,
            ..OfficialStats::default()
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_file(&path, &mut file_stats)
        }));
        if let Err(payload) = outcome {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "unknown panic".to_owned());
            file_stats
                .failures
                .push(format!("{}: panicked: {msg}", path.display()));
        }
        eprintln!(
            "official {}: directives={} modules={} assertions={} failures={}",
            path.file_name().unwrap().to_string_lossy(),
            file_stats.directives,
            file_stats.modules,
            file_stats.assertions,
            file_stats.failures.len()
        );
        stats.directives += file_stats.directives;
        stats.modules += file_stats.modules;
        stats.assertions += file_stats.assertions;
        stats.failures.extend(file_stats.failures);
    }

    eprintln!(
        "official summary: files={} deferred={} directives={} modules={} assertions={} failures={}",
        stats.files,
        stats.deferred,
        stats.directives,
        stats.modules,
        stats.assertions,
        stats.failures.len()
    );
    for (path, reason) in &deferred {
        eprintln!("deferred {}: {reason}", path.display());
    }

    if !stats.failures.is_empty() {
        let mut report = String::from("official spec failures:\n");
        for failure in &stats.failures {
            report.push_str("  ");
            report.push_str(failure);
            report.push('\n');
        }
        panic!("{report}");
    }
    assert!(stats.files > 0, "expected official spec files to run");
}
