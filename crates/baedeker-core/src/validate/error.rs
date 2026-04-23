//! Validation error types.
//!
//! Validation runs after binary decoding and reports type/index/control-flow problems
//! against the decoded module structure.

use alloc::vec::Vec;
use core::fmt;

use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{
    BlockType, DataIdx, ElemIdx, FuncIdx, GlobalIdx, LabelIdx, LocalIdx, MemIdx, RefType, TableIdx,
    TypeIdx, ValType,
};

/// A validation error with byte offset and function context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub offset: ByteOffset,
    pub function: Option<FuncIdx>,
    pub kind: ValidationErrorKind,
}

/// Specific categories of validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationErrorKind {
    UnknownTypeIdx {
        idx: TypeIdx,
    },
    UnknownFuncIdx {
        idx: FuncIdx,
    },
    UndeclaredFuncRef {
        idx: FuncIdx,
    },
    UnknownLocalIdx {
        idx: LocalIdx,
    },
    UnknownGlobalIdx {
        idx: GlobalIdx,
        available: u32,
    },
    UnknownTableIdx {
        idx: TableIdx,
        available: u32,
    },
    UnknownMemIdx {
        idx: MemIdx,
        available: u32,
    },
    UnknownDataIdx {
        idx: DataIdx,
        available: u32,
    },
    UnknownElemIdx {
        idx: ElemIdx,
        available: u32,
    },
    InvalidMemArgAlign {
        op: &'static str,
        max: u32,
        found: u32,
    },
    InvalidSimdLaneIdx {
        op: &'static str,
        max: u8,
        found: u8,
    },
    UnknownLabelIdx {
        idx: LabelIdx,
    },
    Decode {
        context: DecodeContext,
        kind: DecodeErrorKind,
    },
    InvalidGlobalInitExpr,
    NonConstantGlobalInitExpr,
    MutableGlobalInInitExpr {
        idx: GlobalIdx,
    },
    ImmutableGlobalSet {
        idx: GlobalIdx,
    },
    GlobalInitTypeMismatch {
        expected: ValType,
        found: ValType,
    },
    BranchTypeMismatch {
        label: LabelIdx,
        expected: Vec<ValType>,
        found: Vec<ValType>,
    },
    InvalidBrOnNonNullTarget {
        label: LabelIdx,
        found: Vec<ValType>,
    },
    InconsistentBranchTypes {
        expected: Vec<ValType>,
        found: Vec<ValType>,
    },
    UnexpectedElse,
    UnexpectedEnd,
    UnterminatedControlFrames,
    ElseOutsideIf,
    MissingElseForResult,
    InvalidBlockType {
        block_type: BlockType,
    },
    ControlResultTypeMismatch {
        expected: Vec<ValType>,
        found: Vec<ValType>,
    },
    InvalidSelectResultArity {
        found: usize,
    },
    SelectOperandTypeMismatch {
        expected: ValType,
        found: Vec<ValType>,
    },
    StackUnderflow {
        op: &'static str,
        expected: Vec<ValType>,
        available: Vec<ValType>,
    },
    TypeMismatch {
        op: &'static str,
        expected: ValType,
        found: ValType,
    },
    FunctionResultTypeMismatch {
        expected: Vec<ValType>,
        found: Vec<ValType>,
        full_stack: Vec<ValType>,
    },
    ResultTypeMismatch {
        expected: Vec<ValType>,
        found: Vec<ValType>,
    },
    InvalidStartFunctionType {
        params: Vec<ValType>,
        results: Vec<ValType>,
    },
    InvalidElementExpr,
    NonConstantElementExpr,
    ElementExprTypeMismatch {
        expected: ValType,
        found: ValType,
    },
    ElementTableTypeMismatch {
        expected: RefType,
        found: RefType,
    },
    InvalidCallIndirectTableType {
        expected: RefType,
        found: RefType,
    },
    MissingDataCountSection {
        op: &'static str,
    },
    DuplicateExportName {
        name: alloc::string::String,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.function {
            Some(func) => write!(
                f,
                "validation error at byte {} in function {}: {}",
                self.offset.0, func.0, self.kind
            ),
            None => write!(
                f,
                "validation error at byte {}: {}",
                self.offset.0, self.kind
            ),
        }
    }
}

impl fmt::Display for ValidationErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationErrorKind::UnknownTypeIdx { idx } => {
                write!(f, "unknown type index {}", idx.0)
            }
            ValidationErrorKind::UnknownFuncIdx { idx } => {
                write!(f, "unknown function index {}", idx.0)
            }
            ValidationErrorKind::UndeclaredFuncRef { idx } => {
                write!(f, "undeclared function reference {}", idx.0)
            }
            ValidationErrorKind::UnknownLocalIdx { idx } => {
                write!(f, "unknown local index {}", idx.0)
            }
            ValidationErrorKind::UnknownGlobalIdx { idx, available } => {
                write!(
                    f,
                    "unknown global index {} (available globals: {})",
                    idx.0, available
                )
            }
            ValidationErrorKind::UnknownTableIdx { idx, available } => {
                write!(
                    f,
                    "unknown table index {} (available tables: {})",
                    idx.0, available
                )
            }
            ValidationErrorKind::UnknownMemIdx { idx, available } => {
                write!(
                    f,
                    "unknown memory index {} (available memories: {})",
                    idx.0, available
                )
            }
            ValidationErrorKind::UnknownDataIdx { idx, available } => {
                write!(
                    f,
                    "unknown data index {} (available data segments: {})",
                    idx.0, available
                )
            }
            ValidationErrorKind::UnknownElemIdx { idx, available } => {
                write!(
                    f,
                    "unknown element index {} (available element segments: {})",
                    idx.0, available
                )
            }
            ValidationErrorKind::InvalidMemArgAlign { op, max, found } => {
                write!(
                    f,
                    "invalid memarg alignment in {}: found {}, maximum natural alignment exponent {}",
                    op, found, max
                )
            }
            ValidationErrorKind::InvalidSimdLaneIdx { op, max, found } => {
                write!(
                    f,
                    "invalid SIMD lane index in {}: found {}, maximum lane {}",
                    op, found, max
                )
            }
            ValidationErrorKind::UnknownLabelIdx { idx } => {
                write!(f, "unknown label index {}", idx.0)
            }
            ValidationErrorKind::Decode { context, kind } => {
                write!(f, "instruction decode error in {}: {}", context, kind)
            }
            ValidationErrorKind::InvalidGlobalInitExpr => {
                write!(f, "invalid global initializer expression")
            }
            ValidationErrorKind::NonConstantGlobalInitExpr => {
                write!(f, "global initializer must be a constant expression")
            }
            ValidationErrorKind::MutableGlobalInInitExpr { idx } => {
                write!(
                    f,
                    "global initializer references mutable imported global {}",
                    idx.0
                )
            }
            ValidationErrorKind::ImmutableGlobalSet { idx } => {
                write!(f, "cannot assign to immutable global {}", idx.0)
            }
            ValidationErrorKind::GlobalInitTypeMismatch { expected, found } => {
                write!(
                    f,
                    "global initializer type mismatch: expected {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::BranchTypeMismatch {
                label,
                expected,
                found,
            } => {
                write!(
                    f,
                    "branch to label {} has type mismatch: expected {:?}, found {:?}",
                    label.0, expected, found
                )
            }
            ValidationErrorKind::InvalidBrOnNonNullTarget { label, found } => {
                write!(
                    f,
                    "br_on_non_null target label {} must end in a reference type, found {:?}",
                    label.0, found
                )
            }
            ValidationErrorKind::InconsistentBranchTypes { expected, found } => {
                write!(
                    f,
                    "branch targets have inconsistent types: expected {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::UnexpectedElse => write!(f, "unexpected else"),
            ValidationErrorKind::UnexpectedEnd => write!(f, "unexpected end"),
            ValidationErrorKind::UnterminatedControlFrames => {
                write!(f, "unterminated control frames")
            }
            ValidationErrorKind::ElseOutsideIf => write!(f, "else outside if block"),
            ValidationErrorKind::MissingElseForResult => {
                write!(f, "if block with result type requires else branch")
            }
            ValidationErrorKind::InvalidBlockType { block_type } => {
                write!(f, "invalid block type {:?}", block_type)
            }
            ValidationErrorKind::ControlResultTypeMismatch { expected, found } => {
                write!(
                    f,
                    "control frame result type mismatch: expected {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::InvalidSelectResultArity { found } => {
                write!(
                    f,
                    "typed select requires exactly one result type, found {}",
                    found
                )
            }
            ValidationErrorKind::SelectOperandTypeMismatch { expected, found } => {
                write!(
                    f,
                    "select operands must match {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::StackUnderflow {
                op,
                expected,
                available,
            } => {
                write!(
                    f,
                    "operand stack underflow in {}: expected {:?}, available {:?}",
                    op, expected, available
                )
            }
            ValidationErrorKind::TypeMismatch {
                op,
                expected,
                found,
            } => {
                write!(
                    f,
                    "type mismatch in {}: expected {:?}, found {:?}",
                    op, expected, found
                )
            }
            ValidationErrorKind::FunctionResultTypeMismatch {
                expected,
                found,
                full_stack,
            } => {
                write!(
                    f,
                    "function result type mismatch: expected {:?}, found {:?} at stack top (full stack {:?})",
                    expected, found, full_stack
                )
            }
            ValidationErrorKind::ResultTypeMismatch { expected, found } => {
                write!(
                    f,
                    "result type mismatch: expected {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::InvalidStartFunctionType { params, results } => {
                write!(
                    f,
                    "start function must have type [] -> [], found {:?} -> {:?}",
                    params, results
                )
            }
            ValidationErrorKind::InvalidElementExpr => {
                write!(f, "invalid element initializer expression")
            }
            ValidationErrorKind::NonConstantElementExpr => {
                write!(f, "element initializer must be a constant expression")
            }
            ValidationErrorKind::ElementExprTypeMismatch { expected, found } => {
                write!(
                    f,
                    "element initializer type mismatch: expected {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::ElementTableTypeMismatch { expected, found } => {
                write!(
                    f,
                    "active element segment table type mismatch: expected {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::InvalidCallIndirectTableType { expected, found } => {
                write!(
                    f,
                    "call_indirect requires table element type {:?}, found {:?}",
                    expected, found
                )
            }
            ValidationErrorKind::MissingDataCountSection { op } => {
                write!(f, "{} requires a data count section", op)
            }
            ValidationErrorKind::DuplicateExportName { name } => {
                write!(f, "duplicate export name {:?}", name)
            }
        }
    }
}

impl From<DecodeError> for ValidationErrorKind {
    fn from(error: DecodeError) -> Self {
        Self::Decode {
            context: error.context,
            kind: error.kind,
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ValidationError {}

#[cfg(not(feature = "std"))]
impl core::error::Error for ValidationError {}
