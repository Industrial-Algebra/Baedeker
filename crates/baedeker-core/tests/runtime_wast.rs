use std::collections::BTreeMap;
use std::path::Path;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{Value, execute_export};
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

    for directive in wast.directives {
        stats.directives += 1;
        match directive {
            WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat) => {
                stats.modules += 1;
                current_module = Some(lower_wat_module(path, wat));
            }
            WastDirective::AssertReturn { exec, results, .. } => {
                stats.assertions += 1;
                assert_return(path, current_module.as_ref(), exec, results);
            }
            WastDirective::Invoke(invoke) => {
                stats.assertions += 1;
                execute_invoke(path, current_module.as_ref(), invoke);
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
    exec: WastExecute<'_>,
    expected: Vec<WastRet<'_>>,
) {
    let WastExecute::Invoke(invoke) = exec else {
        panic!(
            "{}: runtime assert_return currently supports only invoke execution",
            path.display()
        );
    };
    let actual = execute_invoke(path, module, invoke);
    let expected = expected
        .into_iter()
        .map(|result| expected_value(path, result))
        .collect::<Vec<_>>();

    assert_eq!(
        actual,
        expected,
        "{}: assert_return mismatch",
        path.display()
    );
}

fn execute_invoke(
    path: &Path,
    module: Option<&RegModule>,
    invoke: wast::WastInvoke<'_>,
) -> Vec<Value> {
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

    execute_export(module, invoke.name, &args).unwrap_or_else(|error| {
        panic!(
            "{}: invoke {:?} failed at runtime: {error:?}",
            path.display(),
            invoke.name
        )
    })
}

fn arg_value(path: &Path, arg: WastArg<'_>) -> Value {
    match arg {
        WastArg::Core(WastArgCore::I32(value)) => Value::I32(value),
        WastArg::Core(WastArgCore::I64(value)) => Value::I64(value),
        other => panic!(
            "{}: unsupported runtime WAST argument: {other:?}",
            path.display()
        ),
    }
}

fn expected_value(path: &Path, result: WastRet<'_>) -> Value {
    match result {
        WastRet::Core(WastRetCore::I32(value)) => Value::I32(value),
        WastRet::Core(WastRetCore::I64(value)) => Value::I64(value),
        other => panic!(
            "{}: unsupported runtime WAST expected result: {other:?}",
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
    let stats = run_runtime_wast_dir("runtime");
    assert!(stats.files > 0, "expected runtime WAST fixtures to run");
    assert!(
        stats.assertions > 0,
        "expected runtime WAST fixtures to execute assertions"
    );
}
