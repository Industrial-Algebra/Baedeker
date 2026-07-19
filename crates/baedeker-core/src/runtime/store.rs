//! Mutable runtime state for an instantiated module: linear memories and
//! globals. See [Spec §4.4](https://webassembly.github.io/spec/core/exec/runtime.html).

use alloc::vec::Vec;

use crate::lower::{RegConstInstr, RegElemValue, RegElementMode, RegModule};
use crate::runtime::{RuntimeError, RuntimeErrorKind, RuntimeTrap, Value, trap};
use crate::types::{MemType, TableType};

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
    tables: Vec<Vec<Value>>,
    table_types: Vec<TableType>,
    /// Element segment storage; `None` after the segment is dropped (or was
    /// active/declarative at instantiation).
    elements: Vec<Option<Vec<Value>>>,
    imported_memory_count: u32,
    imported_global_count: u32,
    imported_table_count: u32,
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

        let tables = module
            .tables
            .iter()
            .map(|table| alloc::vec![Value::FuncRef(None); table.limits.min as usize])
            .collect();
        let table_types = module.tables.clone();

        let mut store = Self {
            memories,
            memory_types,
            globals,
            tables,
            table_types,
            elements: alloc::vec![None; module.elements.len()],
            imported_memory_count: module.imported_memory_count,
            imported_global_count: module.imported_global_count,
            imported_table_count: module.imported_table_count,
        };

        // Element segments: active ones are written into their tables (and
        // dropped); passive ones are retained for `table.init`.
        for (idx, segment) in module.elements.iter().enumerate() {
            let values: Vec<Value> = segment
                .values
                .iter()
                .map(|value| match *value {
                    RegElemValue::FuncRef(func) => Value::FuncRef(Some(func.0)),
                    RegElemValue::Null => Value::FuncRef(None),
                })
                .collect();
            match &segment.mode {
                RegElementMode::Active { table, offset } => {
                    let offset = eval_const(offset, &store.globals, store.imported_global_count)?;
                    let Value::I32(offset) = offset else {
                        return Err(RuntimeError {
                            kind: RuntimeErrorKind::InvalidConstExpr,
                        });
                    };
                    let target = store.table_mut(table.0).ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                    })?;
                    let start = offset as usize;
                    let Some(end) = start.checked_add(values.len()) else {
                        return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
                    };
                    if end > target.len() {
                        return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
                    }
                    target[start..end].copy_from_slice(&values);
                }
                RegElementMode::Passive => {
                    store.elements[idx] = Some(values);
                }
                RegElementMode::Dropped => {}
            }
        }

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

    /// Shared access to a defined table by index-space index.
    pub(crate) fn table(&self, idx: u32) -> Option<&Vec<Value>> {
        let defined = idx.checked_sub(self.imported_table_count)? as usize;
        self.tables.get(defined)
    }

    /// Mutable access to a defined table by index-space index.
    pub(crate) fn table_mut(&mut self, idx: u32) -> Option<&mut Vec<Value>> {
        let defined = idx.checked_sub(self.imported_table_count)? as usize;
        self.tables.get_mut(defined)
    }

    /// The declared type of a defined table (for grow limits).
    pub(crate) fn table_type(&self, idx: u32) -> Option<&TableType> {
        let defined = idx.checked_sub(self.imported_table_count)? as usize;
        self.table_types.get(defined)
    }

    /// Whether an index-space table index refers to an imported table.
    pub(crate) fn is_imported_table(&self, idx: u32) -> bool {
        idx < self.imported_table_count
    }

    /// Shared access to a retained element segment.
    pub(crate) fn elem(&self, idx: u32) -> Option<&Option<Vec<Value>>> {
        self.elements.get(idx as usize)
    }

    /// Drop an element segment's storage.
    pub(crate) fn drop_elem(&mut self, idx: u32) -> Option<()> {
        let slot = self.elements.get_mut(idx as usize)?;
        *slot = None;
        Some(())
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
