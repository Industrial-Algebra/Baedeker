// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Validation state and control-flow scaffolding.
//!
//! These types model the operand/control stacks used by the WebAssembly validation
//! algorithm. The full spec algorithm will refine these structures over time.

use alloc::{vec, vec::Vec};

use crate::types::{BlockType, ValType};

/// Operand-stack entry used during validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandType {
    Typed(ValType),
    Bottom,
}

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
    /// A `try_table` block; labels resolve like `Block` (end types).
    TryTable,
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
    pub local_inits: Vec<bool>,
    pub has_else: bool,
    /// The reachability of the ENCLOSING frame at the moment this frame was
    /// pushed. Stack-polymorphic typing is per frame (`push_ctrl` records the
    /// stack height and marks the new block reachable): a block entered in
    /// unreachable code is validated as a reachable block whose operand floor
    /// sits at the enclosing frame's height, and the enclosing frame's
    /// unreachable state is restored when the block ends.
    pub entry_unreachable: bool,
}

/// Operand stack state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeStack {
    values: Vec<OperandType>,
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
        self.values.push(OperandType::Typed(value));
    }

    pub fn push_bottom(&mut self) {
        self.values.push(OperandType::Bottom);
    }

    pub fn pop(&mut self) -> Option<OperandType> {
        self.values.pop()
    }

    /// Reserve capacity for `additional` more operands. Returns `false` when
    /// the allocator refuses, so callers can surface a clean resource error
    /// instead of aborting on infallible growth.
    pub fn try_reserve(&mut self, additional: usize) -> bool {
        self.values.try_reserve(additional).is_ok()
    }

    pub fn as_slice(&self) -> &[OperandType] {
        &self.values
    }
}

/// Function-local validation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationState {
    pub operands: TypeStack,
    pub controls: Vec<ControlFrame>,
    pub locals: Vec<ValType>,
    pub local_inits: Vec<bool>,
    pub reachability: Reachability,
}

impl ValidationState {
    pub fn new(locals: Vec<ValType>, local_inits: Vec<bool>, result_types: Vec<ValType>) -> Self {
        Self {
            operands: TypeStack::new(),
            controls: vec![ControlFrame {
                kind: ControlKind::Function,
                block_type: BlockType::Empty,
                outer_height: 0,
                stack_floor: 0,
                start_types: Vec::new(),
                end_types: result_types,
                // The function frame's local-init snapshot is never restored
                // (`finish_frame` rejects the function frame), so it stays
                // empty and `local_inits` is moved, not copied.
                local_inits: Vec::new(),
                has_else: false,
                entry_unreachable: false,
            }],
            locals,
            local_inits,
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

    /// Push a new control frame, reserving every module-scaled structure
    /// fallibly (the operand stack, the frame's local-init snapshot, and the
    /// control stack). Returns `false` when the allocator refuses, so callers
    /// surface a clean resource error instead of aborting.
    pub fn push_frame(
        &mut self,
        kind: ControlKind,
        block_type: BlockType,
        start_types: Vec<ValType>,
        end_types: Vec<ValType>,
    ) -> bool {
        if !self.operands.try_reserve(start_types.len()) {
            return false;
        }
        let outer_height = self.operands.len();
        for ty in &start_types {
            self.operands.push(*ty);
        }
        let stack_floor = self.operands.len();

        let mut local_inits = Vec::new();
        if local_inits.try_reserve(self.local_inits.len()).is_err() {
            return false;
        }
        local_inits.extend_from_slice(&self.local_inits);

        if self.controls.try_reserve(1).is_err() {
            return false;
        }
        self.controls.push(ControlFrame {
            kind,
            block_type,
            outer_height,
            stack_floor,
            start_types,
            end_types,
            local_inits,
            has_else: false,
            entry_unreachable: self.reachability == Reachability::Unreachable,
        });
        // `push_ctrl` marks the new block reachable: the block's own body is
        // type-checked from its declared parameter types, not from the
        // enclosing frame's polymorphic stack.
        self.reachability = Reachability::Reachable;
        true
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
        let floor = self.current_frame().outer_height;
        self.operands.truncate(floor);
        self.reachability = Reachability::Unreachable;
    }
}
