// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Differential testing against Wasmtime (issue #21).
//!
//! wasm-smith generates valid modules from deterministic seeds; each module
//! is instantiated in both engines and exported functions are called with
//! deterministic argument sets. Results must agree: bit-exact integers,
//! NaN-tolerant floats (NaN payload propagation is
//! implementation-defined), and matching trap kinds.
//!
//! Looping modules are skipped when either engine exhausts its fuel
//! budget — trap/value parity, not termination, is the differential
//! target here.

use baedeker_core::binary::module::Module as BaedekerModule;
use baedeker_core::runtime::{RuntimeErrorKind, Store, Value, execute_export};

/// Modules generated and compared (each × a few argument sets).
const SEEDS: u64 = 256;
/// Per-call fuel budgets (generous; exhaustion just skips the case).
const BAEDEKER_FUEL: u64 = 500_000;
const WASMTIME_FUEL: u64 = 500_000;

#[derive(Debug, PartialEq)]
enum Outcome {
    Values(Vec<Value>),
    Trap(&'static str),
}

fn smith_config() -> wasm_smith::Config {
    // The proposal surface both engines share with Baedeker.
    wasm_smith::Config {
        bulk_memory_enabled: true,
        multi_value_enabled: true,
        simd_enabled: true,
        reference_types_enabled: false,
        tail_call_enabled: false,
        threads_enabled: false,
        memory64_enabled: false,
        max_memories: 1,
        relaxed_simd_enabled: false,
        exceptions_enabled: false,
        gc_enabled: false,
        shared_everything_threads_enabled: false,
        custom_descriptors_enabled: false,
        custom_page_sizes_enabled: false,
        wide_arithmetic_enabled: false,
        max_exports: 8,
        min_exports: 2,
        min_funcs: 2,
        ..Default::default()
    }
}

fn seed_bytes(seed: u64) -> Vec<u8> {
    let mut bytes = seed.to_le_bytes().to_vec();
    bytes.extend_from_slice(&seed.rotate_left(29).to_le_bytes());
    bytes
}

/// Deterministic argument sets for a function type: small edge-flavored
/// values per numeric parameter.
fn arg_sets(ty: &wasmtime::FuncType, variant: usize) -> Vec<wasmtime::Val> {
    const I32S: [i32; 6] = [0, 1, -1, 7, i32::MIN, i32::MAX];
    const I64S: [i64; 6] = [0, 1, -1, 7, i64::MIN, i64::MAX];
    const F32S: [u32; 6] = [
        0x0000_0000, // 0.0
        0x3FC0_0000, // 1.5
        0xBFC0_0000, // -1.5
        0x7F80_0000, // inf
        0xFF80_0000, // -inf
        0x7FC0_0001, // NaN with payload
    ];
    const F64S: [u64; 6] = [
        0x0000_0000_0000_0000, // 0.0
        0x3FF8_0000_0000_0000, // 1.5
        0xBFF8_0000_0000_0000, // -1.5
        0x7FF0_0000_0000_0000, // inf
        0xFFF0_0000_0000_0000, // -inf
        0x7FF8_0000_0000_0001, // NaN with payload
    ];
    ty.params()
        .enumerate()
        .map(|(idx, param)| {
            let pick = (idx + variant) % 6;
            match param {
                wasmtime::ValType::I32 => wasmtime::Val::I32(I32S[pick]),
                wasmtime::ValType::I64 => wasmtime::Val::I64(I64S[pick]),
                wasmtime::ValType::F32 => wasmtime::Val::F32(F32S[pick]),
                wasmtime::ValType::F64 => wasmtime::Val::F64(F64S[pick]),
                _ => wasmtime::Val::I32(0),
            }
        })
        .collect()
}

fn to_baedeker_args(args: &[wasmtime::Val]) -> Vec<Value> {
    args.iter()
        .map(|arg| match arg {
            wasmtime::Val::I32(v) => Value::I32(*v),
            wasmtime::Val::I64(v) => Value::I64(*v),
            wasmtime::Val::F32(v) => Value::F32(f32::from_bits(*v)),
            wasmtime::Val::F64(v) => Value::F64(f64::from_bits(*v)),
            _ => Value::I32(0),
        })
        .collect()
}

fn values_eq(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(a, b)| match (a, b) {
            // NaN payload propagation is implementation-defined.
            (Value::F32(a), Value::F32(b)) => {
                a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
            }
            (Value::F64(a), Value::F64(b)) => {
                a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
            }
            _ => a == b,
        })
}

fn wasmtime_trap_kind(trap: &wasmtime::Trap) -> Option<&'static str> {
    match trap {
        wasmtime::Trap::UnreachableCodeReached => Some("unreachable"),
        wasmtime::Trap::MemoryOutOfBounds => Some("out of bounds memory access"),
        wasmtime::Trap::TableOutOfBounds => Some("out of bounds table access"),
        wasmtime::Trap::IndirectCallToNull => Some("uninitialized element"),
        wasmtime::Trap::BadSignature => Some("indirect call type mismatch"),
        wasmtime::Trap::IntegerDivisionByZero => Some("integer divide by zero"),
        wasmtime::Trap::IntegerOverflow => Some("integer overflow"),
        wasmtime::Trap::BadConversionToInteger => Some("invalid conversion to integer"),
        _ => None,
    }
}

fn baedeker_trap_kind(error: &baedeker_core::runtime::RuntimeError) -> Option<&'static str> {
    match &error.kind {
        RuntimeErrorKind::Trap(trap) => Some(trap.wast_message()),
        _ => None,
    }
}

fn run_wasmtime(bytes: &[u8], name: &str, args: &[wasmtime::Val]) -> Result<Outcome, String> {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    let engine = wasmtime::Engine::new(&config).map_err(|e| e.to_string())?;
    let module = wasmtime::Module::new(&engine, bytes).map_err(|e| e.to_string())?;
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_fuel(WASMTIME_FUEL).map_err(|e| e.to_string())?;
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).map_err(|e| e.to_string())?;
    let func = instance
        .get_func(&mut store, name)
        .ok_or_else(|| format!("no export {name:?}"))?;
    let arity = func.ty(&store).results().len();
    let mut results = vec![wasmtime::Val::I32(0); arity];
    match func.call(&mut store, args, &mut results) {
        Ok(()) => Ok(Outcome::Values(to_baedeker_args(&results))),
        Err(error) => {
            if error.downcast_ref::<wasmtime::Trap>().is_some() {
                let trap = error.downcast::<wasmtime::Trap>().unwrap();
                match wasmtime_trap_kind(&trap) {
                    Some(kind) => Ok(Outcome::Trap(kind)),
                    None => Err(format!("wasmtime unmapped trap {trap:?}")),
                }
            } else if format!("{error:?}").contains("fuel") {
                Err("wasmtime fuel exhausted".into())
            } else {
                Err(format!("wasmtime error {error:?}"))
            }
        }
    }
}

fn run_baedeker(bytes: &[u8], name: &str, args: &[Value]) -> Result<Outcome, String> {
    let module = BaedekerModule::decode(bytes).map_err(|e| format!("decode {e}"))?;
    let lowered = module.lower().map_err(|e| format!("lower {e:?}"))?;
    let store = Store::instantiate(&lowered).map_err(|e| format!("instantiate {e:?}"))?;
    store.set_fuel(Some(BAEDEKER_FUEL));
    match execute_export(&lowered, &store, name, args) {
        Ok(values) => Ok(Outcome::Values(values)),
        Err(error) => {
            if matches!(error.kind, RuntimeErrorKind::FuelExhausted) {
                Err("baedeker fuel exhausted".into())
            } else if let Some(kind) = baedeker_trap_kind(&error) {
                Ok(Outcome::Trap(kind))
            } else {
                Err(format!("baedeker error {:?}", error.kind))
            }
        }
    }
}

#[test]
fn differential_against_wasmtime() {
    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for seed in 0..SEEDS {
        let bytes = seed_bytes(seed);
        let mut unstructured = arbitrary::Unstructured::new(&bytes);
        let Ok(smith_module) = wasm_smith::Module::new(smith_config(), &mut unstructured) else {
            skipped += 1;
            continue;
        };
        let wasm = smith_module.to_bytes();

        // Instantiate once to enumerate numeric-signature exports.
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let Ok(module) = wasmtime::Module::new(&engine, &wasm) else {
            skipped += 1;
            continue;
        };
        let exports: Vec<(String, wasmtime::FuncType)> = module
            .exports()
            .filter_map(|export| match export.ty() {
                wasmtime::ExternType::Func(ty)
                    if ty.params().all(|p| {
                        matches!(
                            p,
                            wasmtime::ValType::I32
                                | wasmtime::ValType::I64
                                | wasmtime::ValType::F32
                                | wasmtime::ValType::F64
                        )
                    }) && ty.results().all(|r| {
                        matches!(
                            r,
                            wasmtime::ValType::I32
                                | wasmtime::ValType::I64
                                | wasmtime::ValType::F32
                                | wasmtime::ValType::F64
                        )
                    }) =>
                {
                    Some((export.name().to_owned(), ty))
                }
                _ => None,
            })
            .take(3)
            .collect();

        for (name, ty) in &exports {
            for variant in 0..2usize {
                let args = arg_sets(ty, variant);
                let b_args = to_baedeker_args(&args);
                let wasmtime_result = run_wasmtime(&wasm, name, &args);
                let baedeker_result = run_baedeker(&wasm, name, &b_args);

                // Fuel exhaustion in either engine: looping module, skip.
                if wasmtime_result
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.contains("fuel"))
                    || baedeker_result
                        .as_ref()
                        .err()
                        .is_some_and(|e| e.contains("fuel"))
                {
                    skipped += 1;
                    continue;
                }

                match (wasmtime_result, baedeker_result) {
                    (Ok(w), Ok(b)) => {
                        let agree = match (&w, &b) {
                            (Outcome::Values(wv), Outcome::Values(bv)) => values_eq(wv, bv),
                            _ => w == b,
                        };
                        if !agree {
                            failures.push(format!(
                                "seed {seed} export {name:?} variant {variant}: wasmtime {w:?} vs baedeker {b:?}"
                            ));
                        } else {
                            compared += 1;
                        }
                    }
                    (w, b) => {
                        failures.push(format!(
                            "seed {seed} export {name:?} variant {variant}: wasmtime {w:?} vs baedeker {b:?}"
                        ));
                    }
                }
            }
        }
    }

    eprintln!(
        "differential: compared={compared} skipped={skipped} failures={}",
        failures.len()
    );
    for failure in failures.iter().take(10) {
        eprintln!("  {failure}");
    }
    assert!(compared > 0, "no comparable executions");
    assert!(
        failures.is_empty(),
        "{} differential failures",
        failures.len()
    );
}
