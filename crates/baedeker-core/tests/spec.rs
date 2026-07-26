use std::path::Path;

use baedeker_core::binary::module::Module;
use baedeker_core::error::DecodeErrorKind;
use baedeker_core::validate::error::{ValidationError, ValidationErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    Valid,
    DecodeError,
    ValidationError,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CaseMeta {
    kind: Option<String>,
    offset: Option<u32>,
    context: Option<String>,
    decode_kind: Option<String>,
}

fn read_case_meta(path: &Path) -> Option<CaseMeta> {
    let meta_path = path.with_extension("meta");
    if !meta_path.exists() {
        return None;
    }

    let mut meta = CaseMeta::default();
    let text = std::fs::read_to_string(&meta_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", meta_path.display()));
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').unwrap_or_else(|| {
            panic!(
                "{}: expected key=value metadata line, found {line:?}",
                meta_path.display()
            )
        });
        match key.trim() {
            "kind" => meta.kind = Some(value.trim().to_owned()),
            "offset" => {
                meta.offset = Some(value.trim().parse().unwrap_or_else(|e| {
                    panic!(
                        "{}: invalid offset {:?}: {e}",
                        meta_path.display(),
                        value.trim()
                    )
                }))
            }
            "context" => meta.context = Some(value.trim().to_owned()),
            "decode_kind" => meta.decode_kind = Some(value.trim().to_owned()),
            other => panic!("{}: unknown metadata key {other:?}", meta_path.display()),
        }
    }

    Some(meta)
}

fn validation_kind_name(kind: &ValidationErrorKind) -> &'static str {
    match kind {
        ValidationErrorKind::UnknownTypeIdx { .. } => "UnknownTypeIdx",
        ValidationErrorKind::UnknownFuncIdx { .. } => "UnknownFuncIdx",
        ValidationErrorKind::UndeclaredFuncRef { .. } => "UndeclaredFuncRef",
        ValidationErrorKind::UnknownLocalIdx { .. } => "UnknownLocalIdx",
        ValidationErrorKind::UninitializedLocal { .. } => "UninitializedLocal",
        ValidationErrorKind::UnknownGlobalIdx { .. } => "UnknownGlobalIdx",
        ValidationErrorKind::UnknownTableIdx { .. } => "UnknownTableIdx",
        ValidationErrorKind::UnknownMemIdx { .. } => "UnknownMemIdx",
        ValidationErrorKind::UnknownDataIdx { .. } => "UnknownDataIdx",
        ValidationErrorKind::UnknownElemIdx { .. } => "UnknownElemIdx",
        ValidationErrorKind::InvalidMemArgAlign { .. } => "InvalidMemArgAlign",
        ValidationErrorKind::InvalidSimdLaneIdx { .. } => "InvalidSimdLaneIdx",
        ValidationErrorKind::UnknownLabelIdx { .. } => "UnknownLabelIdx",
        ValidationErrorKind::Decode { .. } => "Decode",
        ValidationErrorKind::InvalidGlobalInitExpr => "InvalidGlobalInitExpr",
        ValidationErrorKind::NonConstantGlobalInitExpr => "NonConstantGlobalInitExpr",
        ValidationErrorKind::MutableGlobalInInitExpr { .. } => "MutableGlobalInInitExpr",
        ValidationErrorKind::ImmutableGlobalSet { .. } => "ImmutableGlobalSet",
        ValidationErrorKind::GlobalInitTypeMismatch { .. } => "GlobalInitTypeMismatch",
        ValidationErrorKind::BranchTypeMismatch { .. } => "BranchTypeMismatch",
        ValidationErrorKind::InvalidBrOnNonNullTarget { .. } => "InvalidBrOnNonNullTarget",
        ValidationErrorKind::InconsistentBranchTypes { .. } => "InconsistentBranchTypes",
        ValidationErrorKind::UnexpectedElse => "UnexpectedElse",
        ValidationErrorKind::UnexpectedEnd => "UnexpectedEnd",
        ValidationErrorKind::UnterminatedControlFrames => "UnterminatedControlFrames",
        ValidationErrorKind::ElseOutsideIf => "ElseOutsideIf",
        ValidationErrorKind::MissingElseForResult => "MissingElseForResult",
        ValidationErrorKind::InvalidBlockType { .. } => "InvalidBlockType",
        ValidationErrorKind::ControlResultTypeMismatch { .. } => "ControlResultTypeMismatch",
        ValidationErrorKind::InvalidSelectResultArity { .. } => "InvalidSelectResultArity",
        ValidationErrorKind::SelectOperandTypeMismatch { .. } => "SelectOperandTypeMismatch",
        ValidationErrorKind::StackUnderflow { .. } => "StackUnderflow",
        ValidationErrorKind::TypeMismatch { .. } => "TypeMismatch",
        ValidationErrorKind::FunctionResultTypeMismatch { .. } => "FunctionResultTypeMismatch",
        ValidationErrorKind::ResultTypeMismatch { .. } => "ResultTypeMismatch",
        ValidationErrorKind::InvalidStartFunctionType { .. } => "InvalidStartFunctionType",
        ValidationErrorKind::InvalidElementExpr => "InvalidElementExpr",
        ValidationErrorKind::NonConstantElementExpr => "NonConstantElementExpr",
        ValidationErrorKind::ElementExprTypeMismatch { .. } => "ElementExprTypeMismatch",
        ValidationErrorKind::ElementTableTypeMismatch { .. } => "ElementTableTypeMismatch",
        ValidationErrorKind::InvalidCallIndirectTableType { .. } => "InvalidCallIndirectTableType",
        ValidationErrorKind::MissingDataCountSection { .. } => "MissingDataCountSection",
        ValidationErrorKind::DuplicateExportName { .. } => "DuplicateExportName",
        ValidationErrorKind::MemorySizeOutOfRange => "MemorySizeOutOfRange",
        ValidationErrorKind::MemoryMinExceedsMax => "MemoryMinExceedsMax",
    }
}

fn decode_kind_name(kind: &DecodeErrorKind) -> &'static str {
    match kind {
        DecodeErrorKind::UnexpectedEof => "UnexpectedEof",
        DecodeErrorKind::InvalidMagic => "InvalidMagic",
        DecodeErrorKind::UnsupportedVersion { .. } => "UnsupportedVersion",
        DecodeErrorKind::Leb128TooLong => "Leb128TooLong",
        DecodeErrorKind::TooManyLocals => "TooManyLocals",
        DecodeErrorKind::Leb128Overflow => "Leb128Overflow",
        DecodeErrorKind::UnknownSectionId { .. } => "UnknownSectionId",
        DecodeErrorKind::SectionOverflow => "SectionOverflow",
        DecodeErrorKind::SectionOutOfOrder { .. } => "SectionOutOfOrder",
        DecodeErrorKind::DuplicateSection { .. } => "DuplicateSection",
        DecodeErrorKind::UnknownValType { .. } => "UnknownValType",
        DecodeErrorKind::UnknownRefType { .. } => "UnknownRefType",
        DecodeErrorKind::UnknownImportDesc { .. } => "UnknownImportDesc",
        DecodeErrorKind::UnknownExportDesc { .. } => "UnknownExportDesc",
        DecodeErrorKind::InvalidMutability { .. } => "InvalidMutability",
        DecodeErrorKind::InvalidUtf8 => "InvalidUtf8",
        DecodeErrorKind::FunctionCodeLengthMismatch { .. } => "FunctionCodeLengthMismatch",
        DecodeErrorKind::UnknownOpcode { .. } => "UnknownOpcode",
        DecodeErrorKind::UnknownSimdOpcode { .. } => "UnknownSimdOpcode",
        DecodeErrorKind::UnexpectedByte { .. } => "UnexpectedByte",
        DecodeErrorKind::SectionSizeMismatch { .. } => "SectionSizeMismatch",
    }
}

fn decode_context_name(context: &baedeker_core::error::DecodeContext) -> String {
    match context {
        baedeker_core::error::DecodeContext::Magic => "Magic".to_owned(),
        baedeker_core::error::DecodeContext::Version => "Version".to_owned(),
        baedeker_core::error::DecodeContext::SectionHeader => "SectionHeader".to_owned(),
        baedeker_core::error::DecodeContext::SectionBody { id } => format!("SectionBody({id})"),
        baedeker_core::error::DecodeContext::Leb128 => "Leb128".to_owned(),
        baedeker_core::error::DecodeContext::TypeSection => "TypeSection".to_owned(),
        baedeker_core::error::DecodeContext::ImportSection => "ImportSection".to_owned(),
        baedeker_core::error::DecodeContext::FunctionSection => "FunctionSection".to_owned(),
        baedeker_core::error::DecodeContext::TableSection => "TableSection".to_owned(),
        baedeker_core::error::DecodeContext::GlobalSection => "GlobalSection".to_owned(),
        baedeker_core::error::DecodeContext::MemorySection => "MemorySection".to_owned(),
        baedeker_core::error::DecodeContext::ExportSection => "ExportSection".to_owned(),
        baedeker_core::error::DecodeContext::StartSection => "StartSection".to_owned(),
        baedeker_core::error::DecodeContext::ElementSection => "ElementSection".to_owned(),
        baedeker_core::error::DecodeContext::DataSection => "DataSection".to_owned(),
        baedeker_core::error::DecodeContext::DataCountSection => "DataCountSection".to_owned(),
        baedeker_core::error::DecodeContext::CodeSection => "CodeSection".to_owned(),
    }
}

fn assert_validation_meta(path: &Path, err: &ValidationError, meta: &CaseMeta) {
    if let Some(offset) = meta.offset {
        assert_eq!(
            err.offset.0,
            offset as usize,
            "{}: unexpected validation error offset",
            path.display()
        );
    }
    if let Some(kind) = &meta.kind {
        assert_eq!(
            validation_kind_name(&err.kind),
            kind,
            "{}: unexpected validation error kind",
            path.display()
        );
    }
    if meta.context.is_some() || meta.decode_kind.is_some() {
        let ValidationErrorKind::Decode { context, kind } = &err.kind else {
            panic!(
                "{}: decode metadata requires ValidationErrorKind::Decode, found {}",
                path.display(),
                validation_kind_name(&err.kind)
            );
        };
        if let Some(expected_context) = &meta.context {
            assert_eq!(
                decode_context_name(context),
                *expected_context,
                "{}: unexpected wrapped decode context",
                path.display()
            );
        }
        if let Some(expected_kind) = &meta.decode_kind {
            assert_eq!(
                decode_kind_name(kind),
                expected_kind,
                "{}: unexpected wrapped decode error kind",
                path.display()
            );
        }
    }
}

fn assert_decode_meta(path: &Path, err: &baedeker_core::error::DecodeError, meta: &CaseMeta) {
    if let Some(offset) = meta.offset {
        assert_eq!(
            err.offset.0,
            offset as usize,
            "{}: unexpected decode error offset",
            path.display()
        );
    }
    if let Some(kind) = &meta.kind {
        assert_eq!(
            decode_kind_name(&err.kind),
            kind,
            "{}: unexpected decode error kind",
            path.display()
        );
    }
    if let Some(context) = &meta.context {
        assert_eq!(
            decode_context_name(&err.context),
            *context,
            "{}: unexpected decode error context",
            path.display()
        );
    }
    if let Some(kind) = &meta.decode_kind {
        assert_eq!(
            decode_kind_name(&err.kind),
            kind,
            "{}: unexpected decode error kind",
            path.display()
        );
    }
}

fn assert_invalid_case_meta(path: &Path, expected: Expectation) {
    let meta = read_case_meta(path).unwrap_or_else(|| {
        panic!(
            "{}: invalid cases must provide .meta assertions",
            path.display()
        )
    });

    assert!(
        meta.kind.is_some(),
        "{}: invalid-case metadata must specify kind=...",
        path.display()
    );
    assert!(
        meta.offset.is_some(),
        "{}: invalid-case metadata must specify offset=...",
        path.display()
    );

    match expected {
        Expectation::DecodeError => {
            assert!(
                meta.context.is_some(),
                "{}: invalid-decode metadata must specify context=...",
                path.display()
            );
        }
        Expectation::ValidationError => {
            if meta.kind.as_deref() == Some("Decode") {
                assert!(
                    meta.context.is_some(),
                    "{}: decode-preserving invalid-validate metadata must specify context=...",
                    path.display()
                );
                assert!(
                    meta.decode_kind.is_some(),
                    "{}: decode-preserving invalid-validate metadata must specify decode_kind=...",
                    path.display()
                );
            }
        }
        Expectation::Valid => unreachable!("only invalid cases require metadata assertions"),
    }
}

fn run_case(path: &Path, expected: Expectation) {
    let bytes = baedeker_testdata::spec_case_bytes(path);
    let meta = read_case_meta(path);
    match (Module::decode(&bytes), expected) {
        (Ok(module), Expectation::Valid) => {
            module.validate().unwrap_or_else(|e| {
                panic!(
                    "{}: expected valid module, got validation error: {e}",
                    path.display()
                )
            });
        }
        (Ok(module), Expectation::ValidationError) => {
            let err = module.validate().unwrap_err();
            if let Some(meta) = &meta {
                assert_validation_meta(path, &err, meta);
            }
        }
        (Err(err), Expectation::DecodeError) => {
            if let Some(meta) = &meta {
                assert_decode_meta(path, &err, meta);
            }
        }
        (Ok(_), Expectation::DecodeError) => {
            panic!(
                "{}: expected decode error, module decoded successfully",
                path.display()
            )
        }
        (Err(e), Expectation::Valid) => {
            panic!(
                "{}: expected valid module, got decode error: {e}",
                path.display()
            )
        }
        (Err(e), Expectation::ValidationError) => {
            panic!(
                "{}: expected validation error, got decode error: {e}",
                path.display()
            )
        }
    }
}

#[test]
fn spec_valid_groundwork_cases() {
    for path in baedeker_testdata::spec_cases("valid") {
        run_case(&path, Expectation::Valid);
    }
}

#[test]
fn spec_invalid_decode_groundwork_cases() {
    for path in baedeker_testdata::spec_cases("invalid-decode") {
        assert_invalid_case_meta(&path, Expectation::DecodeError);
        run_case(&path, Expectation::DecodeError);
    }
}

#[test]
fn spec_invalid_validate_groundwork_cases() {
    for path in baedeker_testdata::spec_cases("invalid-validate") {
        assert_invalid_case_meta(&path, Expectation::ValidationError);
        run_case(&path, Expectation::ValidationError);
    }
}
