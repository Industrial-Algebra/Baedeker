//! Validation state and control-flow scaffolding.
//!
//! These types model the operand/control stacks used by the WebAssembly validation
//! algorithm. The full spec algorithm will refine these structures over time.

use alloc::{vec, vec::Vec};

use crate::types::{BlockType, ValType};
use crate::validate::error::ValidationErrorKind;

/// Reachability state of the current validation point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    Reachable,
    Unreachable,
}

/// Kind of structured control frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Function,
    Block,
    Loop,
    If,
}

/// A structured control frame in the validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFrame {
    pub kind: ControlKind,
    pub block_type: BlockType,
    pub outer_height: usize,
    pub stack_floor: usize,
    pub start_types: Vec<ValType>,
    pub end_types: Vec<ValType>,
    pub has_else: bool,
}

/// Operand stack state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeStack {
    values: Vec<ValType>,
}

impl TypeStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn truncate(&mut self, len: usize) {
        self.values.truncate(len);
    }

    pub fn push(&mut self, value: ValType) {
        self.values.push(value);
    }

    pub fn pop(&mut self) -> Result<ValType, ValidationErrorKind> {
        self.values.pop().ok_or(ValidationErrorKind::StackUnderflow)
    }

    pub fn as_slice(&self) -> &[ValType] {
        &self.values
    }
}

/// Function-local validation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationState {
    pub operands: TypeStack,
    pub controls: Vec<ControlFrame>,
    pub locals: Vec<ValType>,
    pub reachability: Reachability,
}

impl ValidationState {
    pub fn new(locals: Vec<ValType>, result_types: Vec<ValType>) -> Self {
        Self {
            operands: TypeStack::new(),
            controls: vec![ControlFrame {
                kind: ControlKind::Function,
                block_type: BlockType::Empty,
                outer_height: 0,
                stack_floor: 0,
                start_types: Vec::new(),
                end_types: result_types,
                has_else: false,
            }],
            locals,
            reachability: Reachability::Reachable,
        }
    }

    pub fn current_frame(&self) -> &ControlFrame {
        self.controls
            .last()
            .expect("validation state must always contain a function frame")
    }

    pub fn current_frame_mut(&mut self) -> &mut ControlFrame {
        self.controls
            .last_mut()
            .expect("validation state must always contain a function frame")
    }

    pub fn push_frame(
        &mut self,
        kind: ControlKind,
        block_type: BlockType,
        start_types: Vec<ValType>,
        end_types: Vec<ValType>,
    ) {
        let outer_height = self.operands.len();
        for ty in &start_types {
            self.operands.push(*ty);
        }
        let stack_floor = self.operands.len();

        self.controls.push(ControlFrame {
            kind,
            block_type,
            outer_height,
            stack_floor,
            start_types,
            end_types,
            has_else: false,
        });
    }

    pub fn pop_frame(&mut self) -> Option<ControlFrame> {
        if self.controls.len() > 1 {
            self.controls.pop()
        } else {
            None
        }
    }

    pub fn current_label_types(&self, depth: u32) -> Option<&[ValType]> {
        let frame = self.controls.iter().rev().nth(depth as usize)?;
        Some(match frame.kind {
            ControlKind::Loop => frame.start_types.as_slice(),
            _ => frame.end_types.as_slice(),
        })
    }

    pub fn enter_unreachable(&mut self) {
        let floor = self.current_frame().stack_floor;
        self.operands.truncate(floor);
        self.reachability = Reachability::Unreachable;
    }
}
