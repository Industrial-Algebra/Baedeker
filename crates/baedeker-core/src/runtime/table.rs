// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Sparse-capable table instance storage.
//!
//! Spec-valid tables can declare up to `u32::MAX` entries; a dense
//! `Vec<Value>` for such a table would need ~100 GiB. Tables start dense
//! and migrate to sparse storage (non-default entries only) past a
//! threshold, so a huge table costs memory proportional to the entries
//! actually written — not its declared size.
//!
//! See [Spec §4.2.7](https://webassembly.github.io/spec/core/exec/runtime.html#table-instances).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::runtime::Value;
use crate::types::TableType;

/// Entry count below which a table stays densely allocated. Above this,
/// storage is sparse (`BTreeMap` of non-default entries).
const DENSE_LIMIT: u32 = 1 << 20; // 1M entries ≈ 24 MiB of Values

/// A WebAssembly table instance.
#[derive(Debug, Clone)]
pub struct Table {
    ty: TableType,
    default: Value,
    storage: TableStorage,
}

#[derive(Debug, Clone)]
enum TableStorage {
    Dense(Vec<Value>),
    Sparse {
        len: u32,
        elems: BTreeMap<u32, Value>,
    },
}

impl Table {
    /// Create a table with `min` entries of `default`. Dense allocation is
    /// fallible (`None` on allocation failure); sparse storage never
    /// allocates eagerly.
    pub fn new(ty: TableType, default: Value) -> Option<Self> {
        let min = ty.limits.min;
        let storage = if min <= DENSE_LIMIT {
            let mut vec = Vec::new();
            vec.try_reserve(min as usize).ok()?;
            vec.resize(min as usize, default);
            TableStorage::Dense(vec)
        } else {
            TableStorage::Sparse {
                len: min,
                elems: BTreeMap::new(),
            }
        };
        Some(Self {
            ty,
            default,
            storage,
        })
    }

    /// The table's declared type (limits + element type).
    pub fn ty(&self) -> &TableType {
        &self.ty
    }

    /// Current entry count.
    pub fn len(&self) -> u32 {
        match &self.storage {
            TableStorage::Dense(vec) => vec.len() as u32,
            TableStorage::Sparse { len, .. } => *len,
        }
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read the entry at `idx`, or `None` when out of bounds.
    pub fn get(&self, idx: u32) -> Option<Value> {
        match &self.storage {
            TableStorage::Dense(vec) => vec.get(idx as usize).copied(),
            TableStorage::Sparse { len, elems } => {
                if idx < *len {
                    Some(elems.get(&idx).copied().unwrap_or(self.default))
                } else {
                    None
                }
            }
        }
    }

    /// Write `value` at `idx`; `false` on out of bounds.
    pub fn set(&mut self, idx: u32, value: Value) -> bool {
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                let Some(slot) = vec.get_mut(idx as usize) else {
                    return false;
                };
                *slot = value;
            }
            TableStorage::Sparse { len, elems } => {
                if idx >= *len {
                    return false;
                }
                if value == self.default {
                    elems.remove(&idx);
                } else {
                    elems.insert(idx, value);
                }
            }
        }
        true
    }

    /// Grow by `delta` entries filled with `fill`, returning the previous
    /// length, or `None` when the declared max (or allocation) rejects.
    pub fn grow(&mut self, delta: u32, fill: Value) -> Option<u32> {
        let old = self.len();
        let new = old.checked_add(delta)?;
        if let Some(max) = self.ty.limits.max
            && new > max
        {
            return None;
        }
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                if new <= DENSE_LIMIT {
                    let additional = (new - old) as usize;
                    if vec.try_reserve(additional).is_err() {
                        return None;
                    }
                    vec.resize(new as usize, fill);
                } else {
                    // Migrate to sparse: keep only non-default entries.
                    let mut elems = BTreeMap::new();
                    for (idx, value) in vec.iter().enumerate() {
                        if *value != self.default {
                            elems.insert(idx as u32, *value);
                        }
                    }
                    if fill != self.default {
                        for idx in old..new {
                            elems.insert(idx, fill);
                        }
                    }
                    self.storage = TableStorage::Sparse { len: new, elems };
                }
            }
            TableStorage::Sparse { len, elems } => {
                if fill != self.default {
                    for idx in old..new {
                        elems.insert(idx, fill);
                    }
                }
                *len = new;
            }
        }
        Some(old)
    }

    /// Fill `count` entries starting at `start` with `value`; `false` on
    /// out of bounds.
    pub fn fill(&mut self, start: u32, value: Value, count: u32) -> bool {
        let Some(end) = start.checked_add(count) else {
            return false;
        };
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                if end as usize > vec.len() {
                    return false;
                }
                vec[start as usize..end as usize].fill(value);
            }
            TableStorage::Sparse { len, elems } => {
                if end > *len {
                    return false;
                }
                if value == self.default {
                    for idx in start..end {
                        elems.remove(&idx);
                    }
                } else {
                    for idx in start..end {
                        elems.insert(idx, value);
                    }
                }
            }
        }
        true
    }

    /// Snapshot entries in `start..start + count`; `None` on out of bounds.
    pub fn read_slice(&self, start: u32, count: u32) -> Option<Vec<Value>> {
        let end = start.checked_add(count)?;
        match &self.storage {
            TableStorage::Dense(vec) => {
                if end as usize > vec.len() {
                    return None;
                }
                Some(vec[start as usize..end as usize].to_vec())
            }
            TableStorage::Sparse { len, elems } => {
                if end > *len {
                    return None;
                }
                let mut out = Vec::with_capacity(count as usize);
                for idx in start..end {
                    out.push(elems.get(&idx).copied().unwrap_or(self.default));
                }
                Some(out)
            }
        }
    }

    /// Write `values` starting at `start`; `false` on out of bounds.
    pub fn write_slice(&mut self, start: u32, values: &[Value]) -> bool {
        let Some(end) = start.checked_add(values.len() as u32) else {
            return false;
        };
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                if end as usize > vec.len() {
                    return false;
                }
                vec[start as usize..end as usize].copy_from_slice(values);
            }
            TableStorage::Sparse { len, elems } => {
                if end > *len {
                    return false;
                }
                for (offset, value) in values.iter().enumerate() {
                    let idx = start + offset as u32;
                    if *value == self.default {
                        elems.remove(&idx);
                    } else {
                        elems.insert(idx, *value);
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::types::{Limits, RefType};

    fn funcref_table(min: u32, max: Option<u32>) -> TableType {
        TableType {
            elem: RefType::FuncRef,
            limits: Limits { min, max },
            init: None,
        }
    }

    #[test]
    fn dense_roundtrip_and_grow() {
        let mut table = Table::new(funcref_table(2, Some(8)), Value::FuncRef(None)).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(0), Some(Value::FuncRef(None)));
        assert!(table.set(1, Value::FuncRef(Some((0, 3)))));
        assert_eq!(table.get(1), Some(Value::FuncRef(Some((0, 3)))));
        assert!(!table.set(2, Value::FuncRef(None)));

        assert_eq!(table.grow(2, Value::FuncRef(Some((1, 1)))), Some(2));
        assert_eq!(table.len(), 4);
        assert_eq!(table.get(3), Some(Value::FuncRef(Some((1, 1)))));
        // Declared max rejects growth.
        assert_eq!(table.grow(9, Value::FuncRef(None)), None);
    }

    #[test]
    fn huge_table_is_sparse_and_usable() {
        // u32::MAX entries must not allocate densely.
        let mut table = Table::new(funcref_table(u32::MAX, None), Value::FuncRef(None)).unwrap();
        assert_eq!(table.len(), u32::MAX);
        assert_eq!(table.get(4_000_000_000), Some(Value::FuncRef(None)));
        assert!(table.set(4_000_000_000, Value::FuncRef(Some((2, 5)))));
        assert_eq!(table.get(4_000_000_000), Some(Value::FuncRef(Some((2, 5)))));
        // Writing the default clears the sparse entry.
        assert!(table.set(4_000_000_000, Value::FuncRef(None)));
        assert_eq!(table.get(4_000_000_000), Some(Value::FuncRef(None)));
    }

    #[test]
    fn grow_past_dense_limit_migrates() {
        let mut table = Table::new(funcref_table(4, None), Value::FuncRef(None)).unwrap();
        assert!(table.set(2, Value::FuncRef(Some((7, 7)))));
        let old = table.grow(DENSE_LIMIT, Value::FuncRef(None)).unwrap();
        assert_eq!(old, 4);
        assert!(matches!(table.storage, TableStorage::Sparse { .. }));
        // Contents survive the migration.
        assert_eq!(table.get(2), Some(Value::FuncRef(Some((7, 7)))));
        assert_eq!(table.get(DENSE_LIMIT), Some(Value::FuncRef(None)));
    }

    #[test]
    fn fill_and_slices() {
        let mut table = Table::new(funcref_table(4, None), Value::FuncRef(None)).unwrap();
        assert!(table.fill(1, Value::FuncRef(Some((0, 1))), 2));
        assert_eq!(
            table.read_slice(0, 4).unwrap(),
            vec![
                Value::FuncRef(None),
                Value::FuncRef(Some((0, 1))),
                Value::FuncRef(Some((0, 1))),
                Value::FuncRef(None),
            ]
        );
        assert!(table.write_slice(2, &[Value::FuncRef(Some((9, 9))), Value::FuncRef(None)]));
        assert_eq!(table.get(2), Some(Value::FuncRef(Some((9, 9)))));
        assert_eq!(table.get(3), Some(Value::FuncRef(None)));
        assert!(!table.fill(3, Value::FuncRef(None), 2));
        assert!(table.read_slice(3, 2).is_none());
        assert!(!table.write_slice(3, &[Value::FuncRef(None), Value::FuncRef(None)]));
    }
}
