//! Validation error types.
//!
//! Validation runs after binary decoding and reports type/index/control-flow problems
//! against the decoded module structure.

use alloc::vec::Vec;
use core::fmt;

use crate::error::ByteOffset;
use crate::types::{BlockType, FuncIdx, LabelIdx, LocalIdx, TypeIdx, ValType};

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
    UnknownLabelIdx {
        idx: LabelIdx,
    },
    UnexpectedElse,
    UnexpectedEnd,
    UnterminatedControlFrames,
    ElseOutsideIf,
    MissingElseForResult,
    InvalidBlockType {
        block_type: BlockType,
    },
    StackUnderflow,
    TypeMismatch {
        expected: ValType,
        found: ValType,
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
            ValidationErrorKind::UnknownLabelIdx { idx } => {
                write!(f, "unknown label index {}", idx.0)
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
            ValidationErrorKind::StackUnderflow => write!(f, "operand stack underflow"),
            ValidationErrorKind::TypeMismatch { expected, found } => {
                write!(
                    f,
                    "type mismatch: expected {:?}, found {:?}",
                    expected, found
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

#[cfg(feature = "std")]
impl std::error::Error for ValidationError {}

#[cfg(not(feature = "std"))]
impl core::error::Error for ValidationError {}
