// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::path::Path;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeError, RuntimeErrorKind, Store, Value, execute_export};
use wast::core::{WastArgCore, WastRetCore};
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastRet};

#[derive(Debug, Default)]
struct RuntimeWastStats {
    files: usize,
    directives: usize,
    modules: usize,
    assertions: usize,
    unsupported: BTreeMap<&'static str, usize>,
}

impl RuntimeWastStats {
    fn add(&mut self, other: RuntimeWastStats) {
        self.files += other.files;
        self.directives += other.directives;
        self.modules += other.modules;
        self.assertions += other.assertions;
        for (name, count) in other.unsupported {
            *self.unsupported.entry(name).or_default() += count;
        }
    }

    fn record_unsupported(&mut self, name: &'static str) {
        *self.unsupported.entry(name).or_default() += 1;
    }

    fn unsupported_summary(&self) -> String {
        if self.unsupported.is_empty() {
            return "none".to_owned();
        }

        self.unsupported
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn encode_wat(mut wat: QuoteWat<'_>) -> Result<Vec<u8>, String> {
    wat.encode().map_err(|error| error.to_string())
}

fn run_runtime_wast_dir(subdir: &str) -> RuntimeWastStats {
    let mut total = RuntimeWastStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        let stats = run_runtime_wast_case(&path);
        eprintln!(
            "runtime wast {}: directives={} modules={} assertions={} unsupported={}",
            path.display(),
            stats.directives,
            stats.modules,
            stats.assertions,
            stats.unsupported_summary()
        );
        total.add(stats);
    }

    eprintln!(
        "runtime wast summary [{}]: files={} directives={} modules={} assertions={} unsupported={}",
        subdir,
        total.files,
        total.directives,
        total.modules,
        total.assertions,
        total.unsupported_summary()
    );

    if !total.unsupported.is_empty() {
        panic!(
            "runtime wast [{subdir}] encountered unsupported directives/features: {}",
            total.unsupported_summary()
        );
    }

    total
}

fn run_runtime_wast_case(path: &Path) -> RuntimeWastStats {
    let text = baedeker_testdata::spec_case_text(path);
    let buf = ParseBuffer::new(&text)
        .unwrap_or_else(|error| panic!("{}: failed to parse wast buffer: {error}", path.display()));
    let wast = parse::<Wast<'_>>(&buf).unwrap_or_else(|error| {
        panic!(
            "{}: failed to parse wast directives: {error}",
            path.display()
        )
    });

    let mut stats = RuntimeWastStats {
        files: 1,
        ..RuntimeWastStats::default()
    };
    let mut current_module = None;
    let mut current_store = None;

    for directive in wast.directives {
        stats.directives += 1;
        match directive {
            WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat) => {
                stats.modules += 1;
                let module = lower_wat_module(path, wat);
                current_store = Some(Store::instantiate(&module).unwrap_or_else(|error| {
                    panic!(
                        "{}: expected runtime module to instantiate: {error:?}",
                        path.display()
                    )
                }));
                current_module = Some(module);
            }
            WastDirective::AssertReturn { exec, results, .. } => {
                stats.assertions += 1;
                assert_return(
                    path,
                    current_module.as_ref(),
                    current_store.as_mut(),
                    exec,
                    results,
                );
            }
            WastDirective::AssertTrap { exec, message, .. } => {
                stats.assertions += 1;
                assert_trap(
                    path,
                    current_module.as_ref(),
                    current_store.as_mut(),
                    exec,
                    message,
                );
            }
            WastDirective::AssertExhaustion { call, message, .. } => {
                stats.assertions += 1;
                assert_exhaustion(
                    path,
                    current_module.as_ref(),
                    current_store.as_mut(),
                    call,
                    message,
                );
            }
            WastDirective::Invoke(invoke) => {
                stats.assertions += 1;
                execute_invoke(
                    path,
                    current_module.as_ref(),
                    current_store.as_mut(),
                    invoke,
                );
            }
            other => stats.record_unsupported(directive_name(&other)),
        }
    }

    stats
}

fn lower_wat_module(path: &Path, wat: QuoteWat<'_>) -> RegModule {
    let bytes = encode_wat(wat)
        .unwrap_or_else(|error| panic!("{}: failed to encode module: {error}", path.display()));
    let module = Module::decode(&bytes).unwrap_or_else(|error| {
        panic!(
            "{}: expected runtime module to decode: {error}",
            path.display()
        )
    });
    module.lower().unwrap_or_else(|error| {
        panic!(
            "{}: expected runtime module to lower: {error:?}",
            path.display()
        )
    })
}

fn assert_return(
    path: &Path,
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    exec: WastExecute<'_>,
    expected: Vec<WastRet<'_>>,
) {
    let WastExecute::Invoke(invoke) = exec else {
        panic!(
            "{}: runtime assert_return currently supports only invoke execution",
            path.display()
        );
    };
    let actual = execute_invoke(path, module, store, invoke);

    assert_eq!(
        actual.len(),
        expected.len(),
        "{}: assert_return arity mismatch",
        path.display()
    );
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        assert!(
            result_matches(actual, expected),
            "{}: assert_return mismatch: actual {actual:?}, expected {expected:?}",
            path.display()
        );
    }
}

/// Whether an actual runtime value satisfies an expected WAST result,
/// including NaN patterns.
fn result_matches(actual: &Value, expected: &WastRet<'_>) -> bool {
    match expected {
        WastRet::Core(WastRetCore::I32(value)) => *actual == Value::I32(*value),
        WastRet::Core(WastRetCore::I64(value)) => *actual == Value::I64(*value),
        WastRet::Core(WastRetCore::F32(pattern)) => match (actual, pattern) {
            (Value::F32(actual), wast::core::NanPattern::Value(expected)) => {
                *actual == f32::from_bits(expected.bits)
            }
            (Value::F32(actual), wast::core::NanPattern::CanonicalNan) => {
                actual.to_bits() & 0x7fff_ffff == 0x7fc0_0000
            }
            (Value::F32(actual), wast::core::NanPattern::ArithmeticNan) => actual.is_nan(),
            _ => false,
        },
        WastRet::Core(WastRetCore::F64(pattern)) => match (actual, pattern) {
            (Value::F64(actual), wast::core::NanPattern::Value(expected)) => {
                *actual == f64::from_bits(expected.bits)
            }
            (Value::F64(actual), wast::core::NanPattern::CanonicalNan) => {
                actual.to_bits() & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000
            }
            (Value::F64(actual), wast::core::NanPattern::ArithmeticNan) => actual.is_nan(),
            _ => false,
        },
        WastRet::Core(WastRetCore::V128(pattern)) => {
            let Value::V128(actual) = actual else {
                return false;
            };
            v128_pattern_matches(actual, pattern)
        }
        _ => false,
    }
}

/// Whether a v128 result matches an expected lane pattern (exact lanes for
/// integers, NaN-aware matching for floats).
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

fn assert_exhaustion(
    path: &Path,
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    invoke: wast::WastInvoke<'_>,
    message: &str,
) {
    let error = execute_invoke_result(path, module, store, invoke).expect_err(&format!(
        "{}: assert_exhaustion expected {:?} but invocation returned successfully",
        path.display(),
        message
    ));

    let RuntimeErrorKind::Trap(trap) = error.kind else {
        panic!(
            "{}: assert_exhaustion expected {:?} but got runtime error: {error:?}",
            path.display(),
            message
        );
    };

    assert_eq!(
        trap.wast_message(),
        message,
        "{}: assert_exhaustion message mismatch",
        path.display()
    );
}

fn assert_trap(
    path: &Path,
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    exec: WastExecute<'_>,
    message: &str,
) {
    let WastExecute::Invoke(invoke) = exec else {
        panic!(
            "{}: runtime assert_trap currently supports only invoke execution",
            path.display()
        );
    };

    let error = execute_invoke_result(path, module, store, invoke).expect_err(&format!(
        "{}: assert_trap expected {:?} trap but invocation returned successfully",
        path.display(),
        message
    ));

    let RuntimeErrorKind::Trap(trap) = error.kind else {
        panic!(
            "{}: assert_trap expected {:?} trap but got runtime error: {error:?}",
            path.display(),
            message
        );
    };

    assert_eq!(
        trap.wast_message(),
        message,
        "{}: assert_trap message mismatch",
        path.display()
    );
}

fn execute_invoke(
    path: &Path,
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    invoke: wast::WastInvoke<'_>,
) -> Vec<Value> {
    let name = invoke.name;
    execute_invoke_result(path, module, store, invoke).unwrap_or_else(|error| {
        panic!(
            "{}: invoke {:?} failed at runtime: {error:?}",
            path.display(),
            name
        )
    })
}

fn execute_invoke_result(
    path: &Path,
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    invoke: wast::WastInvoke<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    if invoke.module.is_some() {
        panic!(
            "{}: named-module invoke is not supported by the initial runtime WAST harness",
            path.display()
        );
    }

    let module = module.unwrap_or_else(|| {
        panic!(
            "{}: invoke appeared before any runtime module was defined",
            path.display()
        )
    });
    let args = invoke
        .args
        .into_iter()
        .map(|arg| arg_value(path, arg))
        .collect::<Vec<_>>();

    execute_export(
        module,
        store.expect("store created with module"),
        invoke.name,
        &args,
    )
}

fn arg_value(path: &Path, arg: WastArg<'_>) -> Value {
    match arg {
        WastArg::Core(WastArgCore::I32(value)) => Value::I32(value),
        WastArg::Core(WastArgCore::I64(value)) => Value::I64(value),
        WastArg::Core(WastArgCore::F32(value)) => Value::F32(f32::from_bits(value.bits)),
        WastArg::Core(WastArgCore::F64(value)) => Value::F64(f64::from_bits(value.bits)),
        other => panic!(
            "{}: unsupported runtime WAST argument: {other:?}",
            path.display()
        ),
    }
}

fn directive_name(directive: &WastDirective<'_>) -> &'static str {
    match directive {
        WastDirective::Module(_) => "module",
        WastDirective::ModuleDefinition(_) => "module_definition",
        WastDirective::AssertMalformed { .. } => "assert_malformed",
        WastDirective::AssertInvalid { .. } => "assert_invalid",
        WastDirective::AssertTrap { .. } => "assert_trap",
        WastDirective::AssertReturn { .. } => "assert_return",
        WastDirective::AssertExhaustion { .. } => "assert_exhaustion",
        WastDirective::AssertUnlinkable { .. } => "assert_unlinkable",
        WastDirective::AssertException { .. } => "assert_exception",
        WastDirective::Register { .. } => "register",
        WastDirective::Invoke(_) => "invoke",
        WastDirective::Thread(_) => "thread",
        WastDirective::Wait { .. } => "wait",
        WastDirective::ModuleInstance { .. } => "module_instance",
        WastDirective::AssertSuspension { .. } => "assert_suspension",
    }
}

#[test]
fn runtime_wast_cases() {
    // Deep recursion in fixtures needs a larger host stack than the default
    // test thread provides.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(runtime_wast_cases_inner)
        .expect("spawn runtime wast thread")
        .join()
        .expect("runtime wast thread panicked");
}

fn runtime_wast_cases_inner() {
    let stats = run_runtime_wast_dir("runtime");
    assert!(stats.files > 0, "expected runtime WAST fixtures to run");
    assert!(
        stats.assertions > 0,
        "expected runtime WAST fixtures to execute assertions"
    );
}
