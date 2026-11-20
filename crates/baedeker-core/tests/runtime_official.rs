// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

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
    HostFunction, Imports, RefPattern, RuntimeError, RuntimeErrorKind, Store, Value,
    execute_export, link_func,
};
use baedeker_core::types::{
    CompType, FuncType, GlobalType, MemType, Mutability, NumType, SubType, TableType, TypeIdx,
    ValType, defined_type_matches,
};
use wast::core::{WastArgCore, WastRetCore};
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastRet};

/// Files deferred with a reason (feature not yet implemented). Every entry
/// here is a known gap; keep the list honest and shrinking.
const DEFERRED_FILES: &[(&str, &str)] = &[
    // Guard-page semantics are host/OS-specific.
    (
        "skip-stack-guard-page.wast",
        "stack guard pages not applicable",
    ),
    // The wast parser rejects the confusable unicode names.wast needs.
    ("names.wast", "parser rejects confusable unicode in names"),
    // `thread.wast` is entirely multi-agent: one shared module plus
    // `(thread …)`/`(wait …)` directives whose assertions match `(either …)`.
    // The four-block pre-parse strip cannot make it single-agent.
    (
        "thread.wast",
        "multi-agent behavioral coverage (thread/wait directives) — single-agent engine",
    ),
];

#[derive(Debug, Default)]
struct OfficialStats {
    files: usize,
    deferred: usize,
    directives: usize,
    modules: usize,
    assertions: usize,
    failures: Vec<String>,
    /// Multi-agent `(thread …)` / `(wait …)` directives removed by the
    /// pre-parse strip (the threads corpus' `atomic_wait_notify.wast`).
    threads_directives_skipped: usize,
}

// ── threads directive strip ────────────────────────────────────────

/// Remove the multi-agent directives a single-agent engine cannot execute.
///
/// `atomic_wait_notify.wast` at the threads pin carries exactly FOUR of them
/// (`docs/plans/threads-unit3-contract.md`): two top-level `(thread …)`
/// blocks (its lines 64+) and two `(wait …)` directives, which close those
/// threads. No single-agent directive outside them references their
/// internals, and the wast crate has no multi-agent grammar — so the harness
/// removes the four balanced forms BEFORE parsing. Everything else is passed
/// through untouched (`thread.wast`, whose whole body is multi-agent, is
/// deferred instead).
///
/// Returns the stripped text and the number of forms removed.
fn strip_threads_directives(text: &str) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut stripped = String::with_capacity(text.len());
    let mut kept_from = 0usize;
    let mut removed = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = skip_string(bytes, i),
            b';' if bytes.get(i + 1) == Some(&b';') => i = skip_line_comment(bytes, i),
            b'(' if bytes.get(i + 1) == Some(&b';') => i = skip_block_comment(bytes, i),
            b'(' if is_threads_directive(bytes, i) => match skip_form(bytes, i) {
                Some(end) => {
                    stripped.push_str(&text[kept_from..i]);
                    kept_from = end;
                    removed += 1;
                    i = end;
                }
                // An unbalanced form is left intact — and so is everything
                // after it: with no closing paren, the remainder of the
                // input is INSIDE the form, so any later "top-level"
                // directive is nested, not top-level. Scanning its interior
                // (e.g. stripping a nested `(wait …)` out of an unterminated
                // `(thread …`) would silently mutate a form we promised to
                // preserve for the parser's own span-accurate error. Stop.
                None => {
                    stripped.push_str(&text[kept_from..]);
                    return (stripped, removed);
                }
            },
            _ => i += 1,
        }
    }
    stripped.push_str(&text[kept_from..]);
    (stripped, removed)
}

/// Whether the form opening at `open` is one of the two multi-agent
/// directives. The keyword must be complete — followed by a delimiter, not an
/// identifier continuation — so `(threading …)` is not a `thread` directive.
fn is_threads_directive(bytes: &[u8], open: usize) -> bool {
    let mut i = open + 1;
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    ["thread", "wait"].into_iter().any(|keyword| {
        let end = i + keyword.len();
        bytes.get(i..end) == Some(keyword.as_bytes())
            && matches!(
                bytes.get(end),
                None | Some(b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'"' | b';')
            )
    })
}

/// Index just past the string literal opening at `open` (backslash escapes
/// hide the closing quote).
fn skip_string(bytes: &[u8], open: usize) -> usize {
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Index just past the line comment opening at `open`.
fn skip_line_comment(bytes: &[u8], open: usize) -> usize {
    let mut i = open + 2;
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

/// Index just past the block comment opening at `open` (block comments nest:
/// `document/core/text/lexical.rst`).
fn skip_block_comment(bytes: &[u8], open: usize) -> usize {
    let mut depth = 1usize;
    let mut i = open + 2;
    while i < bytes.len() {
        if bytes[i] == b'(' && bytes.get(i + 1) == Some(&b';') {
            depth += 1;
            i += 2;
        } else if bytes[i] == b';' && bytes.get(i + 1) == Some(&b')') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// Index just past the `)` closing the form opening at `open`, balancing
/// string literals and comments (never counting a paren inside them).
fn skip_form(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = skip_string(bytes, i);
                continue;
            }
            b';' if bytes.get(i + 1) == Some(&b';') => {
                i = skip_line_comment(bytes, i);
                continue;
            }
            b'(' if bytes.get(i + 1) == Some(&b';') => {
                i = skip_block_comment(bytes, i);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

struct Instance {
    module: Rc<RegModule>,
    store: Rc<RefCell<Store>>,
}

struct Registry {
    /// Instances by explicit name (`$id`) and by registered string.
    named: BTreeMap<String, Instance>,
    /// Module definitions from `(module definition $id …)` — lowered but
    /// NOT instantiated; `(module instance $i $id)` re-instantiates them
    /// with fresh state.
    definitions: BTreeMap<String, Rc<RegModule>>,
    last: Option<Instance>,
    /// The link group all instances in this file share.
    group: Rc<RefCell<baedeker_core::runtime::LinkGroup>>,
    /// The exception arena all instances in this file share.
    arena: Rc<RefCell<Vec<baedeker_core::runtime::ExceptionRecord>>>,
    next_instance_id: u32,
}

impl Registry {
    fn new() -> Self {
        Self {
            named: BTreeMap::new(),
            definitions: BTreeMap::new(),
            last: None,
            group: Rc::new(RefCell::new(BTreeMap::new())),
            arena: Rc::new(RefCell::new(Vec::new())),
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
    // Register every provider module's type section and GC heap: concrete
    // reference types in imported signatures are then matched structurally
    // (rolled closed forms) instead of by raw local index, and GC references
    // reaching this instance resolve against the heap that created them.
    // `Imports::with_module_types` is last-wins per module name, so collect
    // the distinct names first.
    let mut provider_names: BTreeMap<&str, ()> = BTreeMap::new();
    for declared in &module.imported_funcs {
        provider_names.insert(declared.module.as_str(), ());
    }
    for declared in &module.imported_memories {
        provider_names.insert(declared.module.as_str(), ());
    }
    for declared in &module.imported_globals {
        provider_names.insert(declared.module.as_str(), ());
    }
    for declared in &module.imported_tables {
        provider_names.insert(declared.module.as_str(), ());
    }
    for declared in &module.imported_tags {
        provider_names.insert(declared.module.as_str(), ());
    }
    for name in provider_names.keys().copied() {
        if let Some(source) = registry.get(Some(name)) {
            imports = imports.with_module_types(name, &source.module);
            imports = imports.provider_heap(source.store.borrow().gc_heap());
        }
    }
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
    // Tag imports: resolve eagerly against registered instances
    // (spectest defines no tags). Binding type mismatches and unknown
    // names are link failures, like the state imports above.
    for declared in &module.imported_tags {
        if declared.module == "spectest" {
            return Err(format!(
                "link: spectest defines no tag export {:?}",
                declared.name
            ));
        }
        let source = registry.get(Some(&declared.module)).ok_or_else(|| {
            format!(
                "link: module {:?} not registered for tag import {:?}",
                declared.module, declared.name
            )
        })?;
        let binding = {
            let store = source.store.borrow();
            store
                .export_tag(&declared.name)
                .and_then(|idx| store.tag_binding(idx).cloned())
        };
        let binding = binding.ok_or_else(|| {
            format!(
                "link: no tag export {:?} on {:?}",
                declared.name, declared.module
            )
        })?;
        if binding.ty.params != declared.ty.params {
            return Err(format!(
                "link: tag type mismatch on {:?}.{:?}: source {:?} vs import {:?}",
                declared.module, declared.name, binding.ty.params, declared.ty.params
            ));
        }
        imports = imports.tag(
            &declared.module,
            &declared.name,
            binding.identity,
            binding.ty.clone(),
            binding.expanded_params.clone(),
            binding.rec.clone(),
            binding.closed.clone(),
        );
    }

    let instance_id = registry.fresh_instance_id();
    let store_rc = Store::instantiate_linked(
        &module,
        &imports,
        &registry.group,
        instance_id,
        &registry.arena,
    )
    .map_err(|error| format_runtime_error(&error))?;
    let mut store_guard = store_rc.borrow_mut();
    let store = &mut *store_guard;

    // Function imports: link lazily against spectest + registered instances.
    // Bound per DECLARATION: one `(module, name)` may be imported several
    // times with different declared types (`type-subtyping.wast`), each
    // matched against the provider's type entry by `match_deftype` above.
    for (func_idx, declared) in module.imported_funcs.iter().enumerate() {
        let host = if declared.module == "spectest" {
            let host = spectest_host_func(&declared.name)?;
            // Spectest imports are matched like any other extern: by the
            // PROVIDER's type entry against this DECLARATION's entry
            // (`eval.ml:1187` runs `Match.match_externtype [] xt' xt` with
            // `xt'` the provider). `host/spectest.ml:35` allocates each host
            // function with `DefT (RecT [SubT (Final, [], FuncT (ts1, ts2))],
            // 0l)`, so the check is `match_deftype` against that single final
            // supertype-free member. Skipping it would let a mismatched
            // declaration bind silently (e.g. `print_i32` declared `(param
            // i64)`).
            if !spectest_signature_matches(&host, &module, declared.type_idx) {
                return Err(format!(
                    "link: incompatible import type on \"spectest\".\"{}\": spectest provides {:?}, import declares type {:?}",
                    declared.name,
                    host.ty(),
                    declared.type_idx
                ));
            }
            host
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
            // (spec: a mismatched type is a link error). `eval.ml:1187` runs
            // `Match.match_externtype [] xt' xt` with `xt'` the PROVIDER's
            // type and `xt` the import DECLARATION; for functions
            // `valid/match.ml:175-186` delegates to `match_deftype`, i.e. the
            // two type ENTRIES must be canonically identical (rolled closed
            // forms — rec-group shape and position included) or the provided
            // one must declare a supertype chain reaching the declared one.
            // Raw local-index comparison would reject structurally equal
            // types; a params/results comparison would accept different
            // rec-group shapes.
            let Some(source_idx) = source
                .module
                .funcs
                .iter()
                .find(|func| func.idx == func_idx)
                .map(|func| func.type_idx)
            else {
                return Err(format!(
                    "link: function {func_idx:?} not found in {:?}",
                    declared.module
                ));
            };
            if !baedeker_core::types::defined_type_matches(
                &source.module.type_defs,
                &source.module.rec_groups,
                source_idx,
                &module.type_defs,
                &module.rec_groups,
                declared.type_idx,
            ) {
                return Err(format!(
                    "link: type mismatch on {:?}.{:?}: source type {:?} vs import type {:?}",
                    declared.module, declared.name, source_idx, declared.type_idx
                ));
            }
            link_func(
                source.module.clone(),
                source.store.clone(),
                func_idx,
                declared.ty.clone(),
            )
        };
        store
            .bind_host_func(func_idx as u32, host)
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
                is64: false,
                shared: false,
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
        "table64" => Ok(TableType {
            elem: baedeker_core::types::RefType::FuncRef,
            limits: baedeker_core::types::Limits {
                min: 10,
                max: Some(20),
                is64: true,
                shared: false,
            },
            init: None,
        }),
        "table" => Ok(TableType {
            elem: baedeker_core::types::RefType::FuncRef,
            limits: baedeker_core::types::Limits {
                min: 10,
                max: Some(20),
                is64: false,
                shared: false,
            },
            init: None,
        }),
        other => Err(format!("unknown spectest table {other:?}")),
    }
}

/// Whether a spectest host function matches an import DECLARATION's type
/// entry, as `match_deftype` (`valid/match.ml:145-150`, reached from
/// `eval.ml:1187`'s `match_externtype`).
///
/// `host/spectest.ml:35` gives every host function the type entry
/// `DefT (RecT [SubT (Final, [], FuncT (ts1, ts2))], 0l)`: one final member,
/// no declared supertypes, no rec-group neighbours. That is exactly the
/// synthetic provider section built here.
fn spectest_signature_matches(host: &HostFunction, module: &RegModule, declared: TypeIdx) -> bool {
    let provided = [SubType {
        is_final: true,
        supertypes: Vec::new(),
        comp: CompType::Func(host.ty().clone()),
    }];
    let provided_recs = [(0u32, 1u32)];
    defined_type_matches(
        &provided,
        &provided_recs,
        TypeIdx(0),
        &module.type_defs,
        &module.rec_groups,
        declared,
    )
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
    // Strip multi-agent directives before parsing (no-op for every file but
    // `atomic_wait_notify.wast`).
    let (text, threads_directives_skipped) = strip_threads_directives(&text);
    stats.threads_directives_skipped = threads_directives_skipped;
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
            WastDirective::Module(wat) => {
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
            WastDirective::ModuleDefinition(wat) => {
                stats.modules += 1;
                // A definition is lowered and stored, never instantiated.
                // Anonymous definitions are decode/validate exercises only.
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lower_wat(path, wat)));
                match outcome {
                    Ok((Some(name), module)) => {
                        registry.definitions.insert(name, Rc::new(module));
                    }
                    Ok((None, _)) => {}
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
            WastDirective::ModuleInstance {
                instance, module, ..
            } => {
                stats.modules += 1;
                // Instantiate a stored definition with FRESH state (new
                // store, new instance id) under the instance name.
                let module_name = module.map(|id| id.name());
                let instance_name = instance.map(|id| id.name());
                let Some(definition) =
                    module_name.and_then(|name| registry.definitions.get(name).cloned())
                else {
                    stats.failures.push(format!(
                        "{}: module instance: no definition for {module_name:?} (instance {instance_name:?})",
                        path.display()
                    ));
                    continue;
                };
                match instantiate(path, &mut registry, (*definition).clone()) {
                    Ok(inst) => registry.insert(instance_name, inst),
                    Err(msg) => stats.failures.push(format!("{}: {msg}", path.display())),
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
                                if !message_matches(&msg, message) {
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
            WastDirective::AssertException { exec, span } => {
                stats.assertions += 1;
                let _ = span;
                assert_exception(path, &registry, exec, stats);
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
            WastArg::Core(WastArgCore::V128(value)) => Value::V128(value.to_le_bytes()),
            WastArg::Core(WastArgCore::RefNull(ty)) => match ty {
                wast::core::HeapType::Abstract {
                    ty: wast::core::AbstractHeapType::Extern,
                    ..
                } => Value::ExternRef(None),
                _ => Value::FuncRef(None),
            },
            // `ref.extern N` builds `ExternRef(HostRef N)`; `ref.host N`
            // builds the any-side `HostRef N` (`script.ml:80`: its type is
            // `AnyHT`). Keeping the zones distinct is what lets an anyref
            // parameter accept a host reference.
            WastArg::Core(WastArgCore::RefExtern(value)) => Value::ExternRef(Some(*value)),
            WastArg::Core(WastArgCore::RefHost(value)) => Value::HostAny(Some(*value)),
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

fn assert_exception(
    path: &Path,
    registry: &Registry,
    exec: WastExecute<'_>,
    stats: &mut OfficialStats,
) {
    let WastExecute::Invoke(invoke) = exec else {
        stats.failures.push(format!(
            "{}: non-invoke assert_exception unsupported",
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
        Err(RuntimeError {
            kind: RuntimeErrorKind::UncaughtException { .. },
        }) => {}
        Err(other) => stats.failures.push(format!(
            "{}: assert_exception expected an escaping exception, got trap {other:?}",
            path.display()
        )),
        Ok(_) => stats.failures.push(format!(
            "{}: assert_exception expected an escaping exception, got a value",
            path.display()
        )),
    }
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
            if expected.len() == 1 && result_matches(&actual, &expected[0], Some(&store)) {
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
            // Reference-pattern matching needs the instance's store (GC object
            // kinds live in the heap, not in the value).
            let store = registry
                .get(invoke.module.map(|id| id.name()))
                .map(|instance| instance.store.borrow());
            let store = store.as_deref();
            if actual.len() != expected.len()
                || !actual
                    .iter()
                    .zip(expected.iter())
                    .all(|(actual, expected)| result_matches(actual, expected, store))
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

/// The pin's assertion-message rule (`interpreter/script/runner.ml:498-502`,
/// `assert_message`): the ACTUAL message must start with the EXPECTED
/// string; the actual may be longer, but never differ inside the expected
/// prefix (`String.sub msg 0 (String.length re) <> re` is the failure case).
fn message_matches(actual: &str, expected: &str) -> bool {
    actual.starts_with(expected)
}

/// The trap-shaped message for an execution error, if the error is a trap in
/// the spec's sense. `ref.cast` failure is a runtime trap whose message
/// starts with `"cast failure, expected "` (`interpreter/exec/eval.ml:657`);
/// `RuntimeErrorKind::CastFailure` stays a distinct embedder-facing kind, but
/// the script harness reports it through the trap-message channel.
fn trap_message(error: &RuntimeError) -> Option<String> {
    match &error.kind {
        RuntimeErrorKind::Trap(trap) => Some(trap.wast_message().to_owned()),
        RuntimeErrorKind::CastFailure { .. } => Some(error.kind.to_string()),
        _ => None,
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
            let Some(actual) = trap_message(&error) else {
                stats.failures.push(format!(
                    "{}: assert_trap expected {message:?}, got non-trap error: {error:?}",
                    path.display()
                ));
                return;
            };
            if !message_matches(&actual, message) {
                stats.failures.push(format!(
                    "{}: assert_trap message mismatch: got {actual:?}, expected {message:?}",
                    path.display()
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
/// including NaN patterns, v128 lane patterns, and reference patterns
/// (`script/runner.ml:464-483` `assert_ref_pat`).
fn result_matches(actual: &Value, expected: &WastRet<'_>, store: Option<&Store>) -> bool {
    match expected {
        WastRet::Core(core) => ret_core_matches(actual, core, store),
        _ => false,
    }
}

fn ret_core_matches(actual: &Value, expected: &WastRetCore<'_>, store: Option<&Store>) -> bool {
    match expected {
        WastRetCore::I32(value) => *actual == Value::I32(*value),
        WastRetCore::I64(value) => *actual == Value::I64(*value),
        WastRetCore::F32(pattern) => match (actual, pattern) {
            (Value::F32(actual), wast::core::NanPattern::Value(expected)) => {
                actual.to_bits() == expected.bits
            }
            (Value::F32(actual), wast::core::NanPattern::CanonicalNan) => {
                actual.to_bits() & 0x7fff_ffff == 0x7fc0_0000
            }
            (Value::F32(actual), wast::core::NanPattern::ArithmeticNan) => actual.is_nan(),
            _ => false,
        },
        WastRetCore::F64(pattern) => match (actual, pattern) {
            (Value::F64(actual), wast::core::NanPattern::Value(expected)) => {
                actual.to_bits() == expected.bits
            }
            (Value::F64(actual), wast::core::NanPattern::CanonicalNan) => {
                actual.to_bits() & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000
            }
            (Value::F64(actual), wast::core::NanPattern::ArithmeticNan) => actual.is_nan(),
            _ => false,
        },
        // Reference patterns: the pinned `assert_ref_pat` rows live in the
        // runtime so embedders and tests share one relation.
        WastRetCore::RefNull(_) => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Null, store)
        }
        WastRetCore::RefExtern(Some(expected)) => baedeker_core::runtime::ref_matches_pattern(
            actual,
            RefPattern::Extern(Some(*expected)),
            store,
        ),
        // Bare `(ref.extern)`: the `RefTypePat ExternHT, _` row matches every
        // reference value, nulls included (runner.ml:475).
        WastRetCore::RefExtern(None) => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Extern(None), store)
        }
        // `(ref.func)` / `(ref.func $f)`.
        WastRetCore::RefFunc(None) => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::FuncAny, store)
        }
        WastRetCore::RefFunc(Some(index)) => match index {
            wast::token::Index::Num(idx, _) => baedeker_core::runtime::ref_matches_pattern(
                actual,
                RefPattern::FuncIndex(*idx),
                store,
            ),
            wast::token::Index::Id(_) => {
                baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::FuncAny, store)
            }
        },
        // `(ref.host n)`: eq_ref identity on the any-side host handle only
        // (`script.ml:93` equates `HostRef` with `HostRef`).
        WastRetCore::RefHost(expected) => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Host(*expected), store)
        }
        WastRetCore::RefAny => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Any, store)
        }
        WastRetCore::RefEq => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Eq, store)
        }
        WastRetCore::RefStruct => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Struct, store)
        }
        WastRetCore::RefArray => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::Array, store)
        }
        WastRetCore::RefI31 | WastRetCore::RefI31Shared => {
            baedeker_core::runtime::ref_matches_pattern(actual, RefPattern::I31, store)
        }
        // `EitherResult`: any alternative may match.
        WastRetCore::Either(cases) => cases
            .iter()
            .any(|case| ret_core_matches(actual, case, store)),
        WastRetCore::V128(pattern) => {
            let Value::V128(actual) = actual else {
                return false;
            };
            v128_pattern_matches(actual, pattern)
        }
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

    // The filter legitimately narrows a run, so the full-count assertions
    // below apply only to an unfiltered run; `failures=0` is unconditional.
    let filter = std::env::var("BAEDEKER_OFFICIAL_FILTER").unwrap_or_default();
    let filtered = !filter.is_empty();

    for path in baedeker_testdata::spec_wast_cases("wast-official") {
        if filtered
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
        stats.threads_directives_skipped += file_stats.threads_directives_skipped;
        stats.failures.extend(file_stats.failures);
    }

    eprintln!(
        "official summary: files={} deferred={} directives={} modules={} assertions={} \
         threads_directives_skipped={} failures={}",
        stats.files,
        stats.deferred,
        stats.directives,
        stats.modules,
        stats.assertions,
        stats.threads_directives_skipped,
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
    assert!(
        stats.files > 0 || filtered,
        "expected official spec files to run"
    );

    // The threads slice's manifest (docs/plans/threads-unit3-contract.md):
    // the 200 pre-existing runnable files + `atomic.wast` +
    // `atomic_wait_notify.wast` + the two pinned deltas. `thread.wast` joins
    // the two pre-existing deferrals. A `BAEDEKER_OFFICIAL_FILTER` run is a
    // narrowing, so these counts are asserted only when it is unset/empty.
    if !filtered {
        assert_eq!(
            stats.files, 204,
            "the threads manifest is 204 runnable files"
        );
        assert_eq!(stats.deferred, 3, "the threads manifest defers 3 files");
        assert_eq!(
            stats.threads_directives_skipped, 4,
            "atomic_wait_notify.wast carries four multi-agent directives \
             (two `thread` blocks, two `wait` directives)"
        );
    }
}

/// `assert_message` accepts an actual message that extends the expected one
/// (the pin's prefix rule) …
#[test]
fn assert_message_prefix_accepts_longer_actual() {
    assert!(message_matches(
        "cast failure, expected (ref null struct) but got i31",
        "cast failure"
    ));
    assert!(message_matches(
        "out of bounds memory access",
        "out of bounds"
    ));
    assert!(message_matches("cast failure", "cast failure"));
}

/// … and rejects an actual message that diverges inside the expected prefix
/// (including one that merely CONTAINS the expected text later on).
#[test]
fn assert_message_prefix_rejects_divergent_actual() {
    assert!(!message_matches("cast failure", "cast failure, expected"));
    assert!(!message_matches("expected cast failure", "cast failure"));
    assert!(!message_matches(
        "call stack exhausted, cast failure",
        "cast failure"
    ));
}

/// A `ref.cast` failure reaches the harness as the trap message the corpus
/// asserts (`eval.ml:657`), while keeping its distinct embedder-facing kind.
#[test]
fn cast_failure_reports_trap_shaped_message() {
    let error = RuntimeError {
        kind: RuntimeErrorKind::CastFailure {
            expected: baedeker_core::types::RefType::from_parts(
                true,
                baedeker_core::types::HeapType::Struct,
            ),
        },
    };
    let message = trap_message(&error).expect("cast failure is trap-shaped");
    assert!(
        message.starts_with("cast failure, expected "),
        "rendered prefix must be exactly `cast failure, expected `: {message}"
    );
    assert!(message_matches(&message, "cast failure"));
    assert!(message_matches(&message, "cast failure, expected"));
}

/// Lower an inline WAT module for a unit-level harness test.
fn inline_module(source: &str) -> RegModule {
    let buf = ParseBuffer::new(source).expect("parse buffer");
    let mut wat = parse::<wast::Wat<'_>>(&buf).expect("parse wat");
    let bytes = wat.encode().expect("encode wat");
    Module::decode(&bytes)
        .expect("decode module")
        .lower()
        .expect("lower module")
}

/// Link `source` against a fresh registry and return the outcome.
fn link_inline(source: &str) -> Result<(), String> {
    let mut registry = Registry::new();
    instantiate(Path::new("<inline>"), &mut registry, inline_module(source)).map(|_| ())
}

/// A spectest declaration is an IMPORT DECLARATION like any other: its type
/// entry must `match_deftype` the host function's own type entry
/// (`host/spectest.ml:35` allocates the host function with `DefT (RecT [SubT
/// (Final, [], FuncT (ts1, ts2))], 0)` and `eval.ml:1187` runs
/// `Match.match_externtype [] xt' xt` on it). A mismatched declaration is a
/// link error — never a silent binding.
#[test]
fn spectest_import_type_mismatch_is_a_link_error() {
    let error = link_inline(r#"(module (import "spectest" "print_i32" (func (param i64))))"#)
        .expect_err("a mismatched spectest declaration must not link");
    assert!(
        error.contains("incompatible import type"),
        "expected an incompatible-import-type link error, got {error}"
    );

    // The matching declaration links.
    link_inline(r#"(module (import "spectest" "print_i32" (func (param i32))))"#)
        .expect("the matching declaration links");
}

/// The spectest check runs PER DECLARATION (the `$t0`/`$t1` shape): two
/// declarations of one name with the same type both bind, and a mismatched
/// declaration is rejected even when a matching one is present.
#[test]
fn spectest_import_type_check_is_per_declaration() {
    link_inline(
        r#"(module
             (import "spectest" "print_i32" (func (param i32)))
             (import "spectest" "print_i32" (func (param i32))))"#,
    )
    .expect("both matching declarations link");

    let error = link_inline(
        r#"(module
             (import "spectest" "print_i32" (func (param i32)))
             (import "spectest" "print_i32" (func (param i64))))"#,
    )
    .expect_err("the mismatched declaration must not link");
    assert!(
        error.contains("incompatible import type"),
        "expected an incompatible-import-type link error, got {error}"
    );
}

// ── pre-parse threads-directive strip (U3) ─────────────────────────

/// The strip removes whole BALANCED forms and nothing else: a paren inside a
/// string or a comment never closes a form early, and a `thread`/`wait`
/// token inside a comment is not a directive. The stripped text still parses
/// — the property the harness relies on.
#[test]
fn strip_threads_directives_preserves_unbalanced_form_and_interior() {
    // Review r1 P2: a form that never balances is preserved INTACT —
    // including its interior and everything after it. The old fallback
    // resumed scanning inside the unterminated form and stripped a
    // nested `(wait …)`, silently mutating the form the safety path
    // promises to hand to the parser unchanged.
    let text = "(thread $T (wait $T)";
    let (stripped, removed) = strip_threads_directives(text);
    assert_eq!(stripped, "(thread $T (wait $T)");
    assert_eq!(removed, 0);
}

#[test]
fn strip_threads_directives_removes_whole_forms_only() {
    let text = r#"(module (memory 1 1))
;; (wait $T0) in a line comment is not a directive
(thread $T1 (shared (module $M))
  (module (func (export "f") (i32.store (i32.const 0) (i32.const 40))))
  (invoke "f")
)
(; (thread $T2 (module)) ;)
(module (data "(\"thread\" )"))
(wait $T1)
(thread $T3 (module $M3 (memory 1 1) (data (i32.const 0) ")")))
(wait $T3)
(module)
"#;
    let (stripped, removed) = strip_threads_directives(text);
    assert_eq!(removed, 4, "two `thread` blocks plus two `wait` directives");
    assert!(
        !stripped.contains("(thread $T1") && !stripped.contains("(thread $T3"),
        "every thread block must be removed: {stripped}"
    );
    assert!(
        !stripped.contains("(wait $T1)") && !stripped.contains("(wait $T3)"),
        "every wait directive must be removed: {stripped}"
    );
    // Decoys survive verbatim: the parenthesised `wait` in a line comment,
    // the parenthesised `thread` in a block comment, and the data string
    // whose escapes and `)` must not be counted as a form end.
    assert!(stripped.contains(";; (wait $T0) in a line comment"));
    assert!(stripped.contains("(; (thread $T2 (module)) ;)"));
    assert!(stripped.contains(r#"(module (data "(\"thread\" )"))"#));
    // Only the forms were removed: the text still parses as a Wast script.
    let buf = ParseBuffer::new(&stripped).expect("stripped text is a valid buffer");
    parse::<Wast<'_>>(&buf).expect("stripped text parses as a Wast script");
}

/// The vendored corpus files other than `atomic_wait_notify.wast` carry no
/// multi-agent directives, so the strip is byte-for-byte a no-op on them.
#[test]
fn strip_threads_directives_is_a_noop_for_the_other_vendored_files() {
    let root = baedeker_testdata::spec_root().join("wast-official");
    for name in [
        "atomic.wast",
        "binary-threads-delta.wast",
        "memory-threads-delta.wast",
    ] {
        let text = baedeker_testdata::spec_case_text(root.join(name));
        let (stripped, removed) = strip_threads_directives(&text);
        assert_eq!(removed, 0, "{name} carries no multi-agent directive");
        assert_eq!(stripped, text, "{name} must be passed through unchanged");
    }
}

/// The vendored corpus file: exactly four balanced blocks, and what is left
/// is a single-agent script (the stock wast grammar cannot parse the four
/// multi-agent forms, which is why the strip runs before parsing).
#[test]
fn strip_threads_directives_removes_the_four_corpus_blocks() {
    let path = baedeker_testdata::spec_root()
        .join("wast-official")
        .join("atomic_wait_notify.wast");
    let text = baedeker_testdata::spec_case_text(&path);
    let (stripped, removed) = strip_threads_directives(&text);
    assert_eq!(
        removed, 4,
        "atomic_wait_notify.wast carries two `thread` blocks and two `wait` directives"
    );
    assert!(!stripped.contains("(thread $T"));
    assert!(!stripped.contains("(wait $T"));
    // The single-agent directives are untouched: the wait/notify exports,
    // the unshared-memory trap assertions, and the OOB/unaligned family.
    assert!(stripped.contains("memory.atomic.wait32"));
    assert!(stripped.contains("memory.atomic.notify"));
    assert!(stripped.contains("expected shared memory"));
    assert!(stripped.contains("unaligned atomic"));

    let buf = ParseBuffer::new(&stripped).expect("stripped corpus is a valid buffer");
    parse::<Wast<'_>>(&buf).expect("the stripped corpus parses as a single-agent script");
}
