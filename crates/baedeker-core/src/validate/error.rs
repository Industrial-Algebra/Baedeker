//! Validation error types.
//!
//! Validation runs after binary decoding and reports type/index/control-flow problems
//! against the decoded module structure.

use alloc::vec::Vec;
use core::fmt;

use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{BlockType, FuncIdx, GlobalIdx, LabelIdx, LocalIdx, MemIdx, TypeIdx, ValType};

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
    UnknownLocalIdx {
        idx: LocalIdx,
    },
    UnknownGlobalIdx {
        idx: GlobalIdx,
    },
    UnknownMemIdx {
        idx: MemIdx,
    },
    UnknownLabelIdx {
        idx: LabelIdx,
    },
    Decode {
        context: DecodeContext,
        kind: DecodeErrorKind,
    },
    BranchTypeMismatch {
        label: LabelIdx,
        expected: Vec<ValType>,
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
            ValidationErrorKind::UnknownLocalIdx { idx } => {
                write!(f, "unknown local index {}", idx.0)
            }
            ValidationErrorKind::UnknownGlobalIdx { idx } => {
                write!(f, "unknown global index {}", idx.0)
            }
            ValidationErrorKind::UnknownMemIdx { idx } => {
                write!(f, "unknown memory index {}", idx.0)
            }
            ValidationErrorKind::UnknownLabelIdx { idx } => {
                write!(f, "unknown label index {}", idx.0)
            }
            ValidationErrorKind::Decode { context, kind } => {
                write!(f, "instruction decode error in {}: {}", context, kind)
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
            ValidationErrorKind::TypeMismatch { expected, found } => {
                write!(
                    f,
                    "type mismatch: expected {:?}, found {:?}",
                    expected, found
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
