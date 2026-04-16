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
                    panic!("{}: invalid offset {:?}: {e}", meta_path.display(), value.trim())
                }))
            }
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
        ValidationErrorKind::TooManyTables { .. } => "TooManyTables",
        ValidationErrorKind::TooManyMemories { .. } => "TooManyMemories",
    }
}

fn decode_kind_name(kind: &DecodeErrorKind) -> &'static str {
    match kind {
        DecodeErrorKind::UnexpectedEof => "UnexpectedEof",
        DecodeErrorKind::InvalidMagic => "InvalidMagic",
        DecodeErrorKind::UnsupportedVersion { .. } => "UnsupportedVersion",
        DecodeErrorKind::Leb128TooLong => "Leb128TooLong",
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

fn assert_validation_meta(path: &Path, err: &ValidationError, meta: &CaseMeta) {
    if let Some(offset) = meta.offset {
        assert_eq!(
            err.offset.0, offset as usize,
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
}

fn assert_decode_meta(path: &Path, err: &baedeker_core::error::DecodeError, meta: &CaseMeta) {
    if let Some(offset) = meta.offset {
        assert_eq!(
            err.offset.0, offset as usize,
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
        run_case(&path, Expectation::DecodeError);
    }
}

#[test]
fn spec_invalid_validate_groundwork_cases() {
    for path in baedeker_testdata::spec_cases("invalid-validate") {
        run_case(&path, Expectation::ValidationError);
    }
}
