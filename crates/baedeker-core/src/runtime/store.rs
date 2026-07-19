//! Mutable runtime state for an instantiated module: linear memories and
//! globals. See [Spec §4.4](https://webassembly.github.io/spec/core/exec/runtime.html).

use alloc::vec::Vec;

use crate::lower::{RegConstInstr, RegModule};
use crate::runtime::{RuntimeError, RuntimeErrorKind, RuntimeTrap, Value, trap};
use crate::types::MemType;

/// Size of one WebAssembly memory page in bytes.
pub const PAGE_SIZE: usize = 65536;

/// Mutable state of an instantiated module.
///
/// Defined memories and globals only; the imported index-space prefix is
/// tracked so accesses to imported memories/globals can fail explicitly.
#[derive(Debug)]
pub struct Store {
    memories: Vec<Vec<u8>>,
    memory_types: Vec<MemType>,
    globals: Vec<Value>,
    imported_memory_count: u32,
    imported_global_count: u32,
}

impl Store {
    /// Instantiate a lowered module: zero memories to their minimum size,
    /// evaluate global initializers in declaration order, then apply active
    /// data segments.
    pub fn instantiate(module: &RegModule) -> Result<Self, RuntimeError> {
        let mut globals = Vec::with_capacity(module.globals.len());
        for global in &module.globals {
            globals.push(eval_const(
                &global.init,
                &globals,
                module.imported_global_count,
            )?);
        }

        let memories = module
            .memories
            .iter()
            .map(|mem| alloc::vec![0u8; mem.limits.min as usize * PAGE_SIZE])
            .collect();
        let memory_types = module.memories.clone();

        let mut store = Self {
            memories,
            memory_types,
            globals,
            imported_memory_count: module.imported_memory_count,
            imported_global_count: module.imported_global_count,
        };

        for segment in &module.data {
            let offset = eval_const(&segment.offset, &store.globals, store.imported_global_count)?;
            let Value::I32(offset) = offset else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::InvalidConstExpr,
                });
            };
            let memory = store.memory_mut(segment.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: segment.memory.0,
                },
            })?;
            let start = offset as usize;
            let Some(end) = start.checked_add(segment.bytes.len()) else {
                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
            };
            if end > memory.len() {
                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
            }
            memory[start..end].copy_from_slice(&segment.bytes);
        }

        Ok(store)
    }

    /// Mutable access to a defined memory by index-space index, or `None`
    /// when the index is imported or out of range.
    pub(crate) fn memory_mut(&mut self, idx: u32) -> Option<&mut Vec<u8>> {
        let defined = idx.checked_sub(self.imported_memory_count)? as usize;
        self.memories.get_mut(defined)
    }

    /// The declared type of a defined memory (for grow limits).
    pub(crate) fn memory_type(&self, idx: u32) -> Option<&MemType> {
        let defined = idx.checked_sub(self.imported_memory_count)? as usize;
        self.memory_types.get(defined)
    }

    /// Read a defined global by index-space index.
    pub(crate) fn global(&self, idx: u32) -> Option<Value> {
        let defined = idx.checked_sub(self.imported_global_count)? as usize;
        self.globals.get(defined).copied()
    }

    /// Write a defined global by index-space index.
    pub(crate) fn set_global(&mut self, idx: u32, value: Value) -> Option<()> {
        let defined = idx.checked_sub(self.imported_global_count)? as usize;
        let slot = self.globals.get_mut(defined)?;
        *slot = value;
        Some(())
    }

    /// Whether an index-space memory index refers to an imported memory.
    pub(crate) fn is_imported_memory(&self, idx: u32) -> bool {
        idx < self.imported_memory_count
    }

    /// Whether an index-space global index refers to an imported global.
    pub(crate) fn is_imported_global(&self, idx: u32) -> bool {
        idx < self.imported_global_count
    }

    /// Read a defined global's current value (embedding/debug access).
    pub fn get_global(&self, idx: u32) -> Option<Value> {
        self.global(idx)
    }

    /// Read a defined memory's bytes (embedding/debug access).
    pub fn get_memory(&self, idx: u32) -> Option<&[u8]> {
        let defined = idx.checked_sub(self.imported_memory_count)? as usize;
        self.memories.get(defined).map(Vec::as_slice)
    }
}

/// Evaluate a lowered const expression against the globals initialized so
/// far. Used for global initializers and data segment offsets.
fn eval_const(
    expr: &[RegConstInstr],
    globals: &[Value],
    imported_global_count: u32,
) -> Result<Value, RuntimeError> {
    let mut stack: Vec<Value> = Vec::new();
    for instr in expr {
        match *instr {
            RegConstInstr::I32Const(value) => stack.push(Value::I32(value)),
            RegConstInstr::I64Const(value) => stack.push(Value::I64(value)),
            RegConstInstr::F32Const(bits) => stack.push(Value::F32(f32::from_bits(bits))),
            RegConstInstr::F64Const(bits) => stack.push(Value::F64(f64::from_bits(bits))),
            RegConstInstr::GlobalGet(global) => {
                if global.0 < imported_global_count {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::ImportedGlobalAccessUnsupported {
                            global: global.0,
                        },
                    });
                }
                let defined = (global.0 - imported_global_count) as usize;
                let value = globals.get(defined).copied().ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
                })?;
                stack.push(value);
            }
            RegConstInstr::I32Add => {
                let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                stack.push(Value::I32(lhs.wrapping_add(rhs)));
            }
            RegConstInstr::I32Sub => {
                let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                stack.push(Value::I32(lhs.wrapping_sub(rhs)));
            }
            RegConstInstr::I32Mul => {
                let (lhs, rhs) = pop_i32_pair(&mut stack)?;
                stack.push(Value::I32(lhs.wrapping_mul(rhs)));
            }
            RegConstInstr::I64Add => {
                let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                stack.push(Value::I64(lhs.wrapping_add(rhs)));
            }
            RegConstInstr::I64Sub => {
                let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                stack.push(Value::I64(lhs.wrapping_sub(rhs)));
            }
            RegConstInstr::I64Mul => {
                let (lhs, rhs) = pop_i64_pair(&mut stack)?;
                stack.push(Value::I64(lhs.wrapping_mul(rhs)));
            }
        }
    }

    if stack.len() != 1 {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    }
    Ok(stack[0])
}

fn pop_i32_pair(stack: &mut Vec<Value>) -> Result<(i32, i32), RuntimeError> {
    let (Some(Value::I32(rhs)), Some(Value::I32(lhs))) = (stack.pop(), stack.pop()) else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    };
    Ok((lhs, rhs))
}

fn pop_i64_pair(stack: &mut Vec<Value>) -> Result<(i64, i64), RuntimeError> {
    let (Some(Value::I64(rhs)), Some(Value::I64(lhs))) = (stack.pop(), stack.pop()) else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidConstExpr,
        });
    };
    Ok((lhs, rhs))
}
