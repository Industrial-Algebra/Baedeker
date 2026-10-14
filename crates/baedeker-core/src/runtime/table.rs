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

use alloc::vec::Vec;

use crate::runtime::Value;
use crate::types::TableType;

/// Entry count below which a table stays densely allocated. Above this,
/// storage is sparse (`BTreeMap` of non-default entries).
const DENSE_LIMIT: u32 = 1 << 20; // 1M entries ≈ 24 MiB of Values

/// The uniform-fill value covering `idx`, if any.
fn fill_value(fills: &[(u32, u32, Value)], idx: u32) -> Option<Value> {
    for &(start, end, value) in fills {
        if idx >= start && idx < end {
            return Some(value);
        }
    }
    None
}

/// Record a migration run: two-and-longer runs become fill ranges,
/// single entries materialize explicitly.
fn push_run(
    elems: &mut Vec<(u32, Value)>,
    fills: &mut Vec<(u32, u32, Value)>,
    start: u32,
    end: u32,
    value: Value,
) {
    if end - start > 1 {
        fills.push((start, end, value));
    } else {
        entry_upsert(elems, start, value);
    }
}

/// Overwrite `[start, end)` with a uniform non-default `value`: split or
/// drop intersecting ranges, then splice in the new range (merging with
/// equal-valued neighbors when contiguous).
fn overwrite_fills(fills: &mut Vec<(u32, u32, Value)>, start: u32, end: u32, value: Value) {
    let mut rewritten: Vec<(u32, u32, Value)> = Vec::with_capacity(fills.len() + 1);
    for &(fstart, fend, fval) in fills.iter() {
        if fend <= start || fstart >= end {
            rewritten.push((fstart, fend, fval));
        } else {
            if fstart < start {
                rewritten.push((fstart, start, fval));
            }
            if fend > end {
                rewritten.push((end, fend, fval));
            }
        }
    }
    // Splice the new range in, merging with contiguous equal neighbors.
    let mut result: Vec<(u32, u32, Value)> = Vec::with_capacity(rewritten.len() + 1);
    let mut pending = Some((start, end, value));
    for &(fstart, fend, fval) in rewritten.iter() {
        match pending {
            Some((pstart, pend, pval)) if fstart <= pend && pstart <= fend && fval == pval => {
                pending = Some((pstart.min(fstart), pend.max(fend), pval));
            }
            Some((pstart, pend, pval)) if fstart > pend => {
                result.push((pstart, pend, pval));
                pending = None;
                result.push((fstart, fend, fval));
            }
            Some((pstart, pend, pval)) => {
                result.push((fstart, fend, fval));
                pending = Some((pstart, pend, pval));
            }
            None => result.push((fstart, fend, fval)),
        }
    }
    if let Some((pstart, pend, pval)) = pending {
        result.push((pstart, pend, pval));
    }
    // Re-sort: neighbors merged above keep order, but be defensive.
    result.sort_by_key(|&(start, _, _)| start);
    *fills = result;
}

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
        /// Explicit entries, sorted by index: single writes, holes
        /// punched into fill ranges, and (rare) migration singletons.
        /// A sorted Vec (not a map) so bulk construction can reserve
        /// fallibly — migration must fail cleanly, never abort.
        elems: Vec<(u32, Value)>,
        /// Disjoint, sorted uniform-fill ranges `[start, end)` carrying a
        /// non-default value, created by grow/fill/copy bulk writes.
        /// Explicit `elems` entries take precedence (including explicit
        /// default entries, which act as holes punched into a range), so
        /// bulk writes never materialize per-entry storage.
        fills: Vec<(u32, u32, Value)>,
    },
}

/// Sorted-Vec entry lookup.
fn entry_get(elems: &[(u32, Value)], idx: u32) -> Option<Value> {
    elems
        .binary_search_by_key(&idx, |&(key, _)| key)
        .ok()
        .map(|pos| elems[pos].1)
}

/// Sorted-Vec upsert.
fn entry_upsert(elems: &mut Vec<(u32, Value)>, idx: u32, value: Value) {
    match elems.binary_search_by_key(&idx, |&(key, _)| key) {
        Ok(pos) => elems[pos].1 = value,
        Err(pos) => elems.insert(pos, (idx, value)),
    }
}

/// Sorted-Vec removal.
fn entry_remove(elems: &mut Vec<(u32, Value)>, idx: u32) -> Option<Value> {
    match elems.binary_search_by_key(&idx, |&(key, _)| key) {
        Ok(pos) => Some(elems.remove(pos).1),
        Err(_) => None,
    }
}

impl Table {
    /// Create a table with `min` entries of `default`. Dense allocation is
    /// fallible (`None` on allocation failure); sparse storage never
    /// allocates eagerly.
    pub fn new(ty: TableType, default: Value) -> Option<Self> {
        // Table storage is still 32-bit; a 64-bit min beyond the u32 range is
        // not representable here and is reported as an allocation failure.
        // 64-bit table indexing is the runtime unit's job.
        let min = u32::try_from(ty.limits.min).ok()?;
        let storage = if min <= DENSE_LIMIT {
            let mut vec = Vec::new();
            vec.try_reserve(min as usize).ok()?;
            vec.resize(min as usize, default);
            TableStorage::Dense(vec)
        } else {
            TableStorage::Sparse {
                len: min,
                elems: Vec::new(),
                fills: Vec::new(),
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
    pub fn len(&self) -> u64 {
        match &self.storage {
            TableStorage::Dense(vec) => vec.len() as u64,
            TableStorage::Sparse { len, .. } => u64::from(*len),
        }
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read the entry at `idx`, or `None` when out of bounds.
    pub fn get(&self, idx: u64) -> Option<Value> {
        match &self.storage {
            TableStorage::Dense(vec) => vec.get(usize::try_from(idx).ok()?).copied(),
            TableStorage::Sparse { len, elems, fills } => {
                if idx < u64::from(*len) {
                    // `idx < len <= u32::MAX`, so the narrowing is exact.
                    let idx = idx as u32;
                    Some(
                        entry_get(elems, idx)
                            .or_else(|| fill_value(fills, idx))
                            .unwrap_or(self.default),
                    )
                } else {
                    None
                }
            }
        }
    }

    /// Write `value` at `idx`; `false` on out of bounds.
    pub fn set(&mut self, idx: u64, value: Value) -> bool {
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                let Ok(idx) = usize::try_from(idx) else {
                    return false;
                };
                let Some(slot) = vec.get_mut(idx) else {
                    return false;
                };
                *slot = value;
            }
            TableStorage::Sparse { len, elems, fills } => {
                if idx >= u64::from(*len) {
                    return false;
                }
                let idx = idx as u32;
                if value == self.default {
                    entry_remove(elems, idx);
                    // Whenever a fill range covers the index, an explicit
                    // default entry must REMAIN (or be recorded) as a
                    // hole — otherwise the range value resurfaces. This
                    // is independent of whether an explicit entry existed
                    // before (review r2 P1: set-to-B then set-to-default
                    // must land on default, not the fill value).
                    if fill_value(fills, idx).is_some() {
                        entry_upsert(elems, idx, self.default);
                    }
                } else {
                    entry_upsert(elems, idx, value);
                }
            }
        }
        true
    }

    /// Grow by `delta` entries filled with `fill`, returning the previous
    /// length, or `None` when the declared max (or allocation) rejects.
    pub fn grow(&mut self, delta: u64, fill: Value) -> Option<u64> {
        let old = self.len();
        let new = old.checked_add(delta)?;
        if let Some(max) = self.ty.limits.max
            && new > max
        {
            return None;
        }
        // Internal storage is bounded to `u32::MAX` entries; a larger growth
        // is an allocation failure (yields -1, never aborts).
        let new = u32::try_from(new).ok()?;
        let old = u32::try_from(old).ok()?;
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                if new <= DENSE_LIMIT {
                    let additional = (new - old) as usize;
                    if vec.try_reserve(additional).is_err() {
                        return None;
                    }
                    vec.resize(new as usize, fill);
                } else {
                    // Migrate to sparse: keep only non-default entries,
                    // compressing runs of equal values into fill ranges.
                    // Build both structures up front and commit only when
                    // reservations succeed — a fragmented table (many
                    // singleton runs) must fail the grow cleanly, never
                    // abort (review r2 P1).
                    let mut elems: Vec<(u32, Value)> = Vec::new();
                    let mut fills = Vec::new();
                    let mut run: Option<(u32, u32, Value)> = None;
                    for (idx, value) in vec.iter().enumerate() {
                        if *value != self.default {
                            let idx = idx as u32;
                            match run {
                                Some((_, ref mut end, ref mut v))
                                    if *v == *value && *end == idx =>
                                {
                                    *end = idx + 1;
                                }
                                Some((start, end, v)) => {
                                    push_run(&mut elems, &mut fills, start, end, v);
                                    run = Some((idx, idx + 1, *value));
                                }
                                None => run = Some((idx, idx + 1, *value)),
                            }
                        }
                    }
                    if let Some((start, end, v)) = run {
                        push_run(&mut elems, &mut fills, start, end, v);
                    }
                    if fill != self.default && new > old {
                        fills.push((old, new, fill));
                    }
                    // The dense storage stays intact until the sparse
                    // structures are provably allocatable.
                    let elems_cap = elems.len().max(1);
                    let fills_cap = fills.len().max(1);
                    if elems.try_reserve(elems_cap).is_err()
                        || fills.try_reserve(fills_cap).is_err()
                    {
                        return None;
                    }
                    self.storage = TableStorage::Sparse {
                        len: new,
                        elems,
                        fills,
                    };
                }
            }
            TableStorage::Sparse { len, fills, .. } => {
                // Growing by a uniform value appends ONE range: O(1), no
                // per-entry allocation (grow must fail cleanly or succeed,
                // never abort).
                if fill != self.default && new > old {
                    let merged = matches!(
                        fills.last(),
                        Some((_, end, v)) if *end == old && *v == fill
                    );
                    if merged {
                        if let Some(last) = fills.last_mut() {
                            last.1 = new;
                        }
                    } else {
                        fills.push((old, new, fill));
                    }
                }
                *len = new;
            }
        }
        Some(u64::from(old))
    }

    /// Fill `count` entries starting at `start` with `value`; `false` on
    /// out of bounds.
    pub fn fill(&mut self, start: u64, value: Value, count: u64) -> bool {
        let Some(end) = start.checked_add(count) else {
            return false;
        };
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
                    return false;
                };
                if end > vec.len() {
                    return false;
                }
                vec[start..end].fill(value);
            }
            TableStorage::Sparse { len, elems, fills } => {
                if end > u64::from(*len) {
                    return false;
                }
                // `end <= len <= u32::MAX`, so both narrowings are exact.
                let (start, end) = (start as u32, end as u32);
                // Drop explicit entries in the span; they are overwritten.
                elems.retain(|&(idx, _)| idx < start || idx >= end);
                if value == self.default {
                    // Punch the span out of any intersecting ranges.
                    let mut rewritten = Vec::new();
                    for &(fstart, fend, fval) in fills.iter() {
                        if fend <= start || fstart >= end {
                            rewritten.push((fstart, fend, fval));
                        } else {
                            if fstart < start {
                                rewritten.push((fstart, start, fval));
                            }
                            if fend > end {
                                rewritten.push((end, fend, fval));
                            }
                        }
                    }
                    *fills = rewritten;
                } else {
                    overwrite_fills(fills, start, end, value);
                }
            }
        }
        true
    }

    /// Same-table `table.copy`: `[dst, dst+count)` receives
    /// `[src, src+count)`. Both ranges are bounds-checked first; when the
    /// ranges overlap the copy direction preserves the source data, and
    /// uniform source runs are moved as fill ranges rather than
    /// materialized per entry.
    pub fn copy_within(&mut self, dst: u64, src: u64, count: u64) -> bool {
        let Some(src_end) = src.checked_add(count) else {
            return false;
        };
        let Some(dst_end) = dst.checked_add(count) else {
            return false;
        };
        if src_end > self.len() || dst_end > self.len() {
            return false;
        }
        if count == 0 {
            return true;
        }
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                let (Ok(dst), Ok(src), Ok(count)) = (
                    usize::try_from(dst),
                    usize::try_from(src),
                    usize::try_from(count),
                ) else {
                    return false;
                };
                // `copy_within(src_range, dest_start)`: the range is the
                // SOURCE; the destination is the second argument.
                vec.copy_within(src..src + count, dst);
            }
            TableStorage::Sparse { .. } => {
                // Process runs in an order that never overwrites
                // not-yet-read source data: forwards when the destination
                // sits below the source (or they do not overlap), backwards
                // otherwise.
                let forward = dst <= src || dst_end <= src;
                let mut idx = if forward { src } else { src_end - 1 };
                loop {
                    let value = match self.get(idx) {
                        Some(value) => value,
                        None => return false,
                    };
                    // Extend the run while the next index (in iteration
                    // order) holds the same value.
                    let step = if forward {
                        let mut run_end = idx + 1;
                        while run_end < src_end && self.get(run_end) == Some(value) {
                            run_end += 1;
                        }
                        run_end - idx
                    } else {
                        let mut run_start = idx;
                        while run_start > src && self.get(run_start - 1) == Some(value) {
                            run_start -= 1;
                        }
                        idx - run_start + 1
                    };
                    let (run_start, run_len) = if forward {
                        (idx, step)
                    } else {
                        (idx + 1 - step, step)
                    };
                    let dst_start = dst + (run_start - src);
                    self.overwrite_range(dst_start, run_len, value);
                    if forward {
                        if run_start + step >= src_end {
                            break;
                        }
                        idx = run_start + step;
                    } else {
                        if run_start == src {
                            break;
                        }
                        idx = run_start - 1;
                    }
                }
            }
        }
        true
    }

    /// Cross-table `table.copy` (no overlap possible): `[dst, dst+count)`
    /// of `self` receives `[src, src+count)` of `source`. Bounds-checked
    /// as a whole first; uniform source runs move as fill ranges.
    pub fn copy_from(&mut self, source: &Table, dst: u64, src: u64, count: u64) -> bool {
        let Some(src_end) = src.checked_add(count) else {
            return false;
        };
        let Some(dst_end) = dst.checked_add(count) else {
            return false;
        };
        if src_end > source.len() || dst_end > self.len() {
            return false;
        }
        if count == 0 {
            return true;
        }
        let mut idx = src;
        while idx < src_end {
            let value = source.get(idx).expect("bounds checked");
            let mut run_end = idx + 1;
            while run_end < src_end && source.get(run_end) == Some(value) {
                run_end += 1;
            }
            let dst_start = dst + (idx - src);
            self.overwrite_range(dst_start, run_end - idx, value);
            idx = run_end;
        }
        true
    }

    /// Overwrite `[start, start+count)` with a uniform `value` (bounds
    /// pre-checked by callers when they need whole-range checking; this
    /// helper also guards per-range to keep invariants local).
    fn overwrite_range(&mut self, start: u64, count: u64, value: Value) {
        let Some(end) = start.checked_add(count) else {
            return;
        };
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
                    return;
                };
                if end <= vec.len() {
                    vec[start..end].fill(value);
                }
            }
            TableStorage::Sparse { len, elems, fills } => {
                if end > u64::from(*len) {
                    return;
                }
                let (start, end) = (start as u32, end as u32);
                elems.retain(|&(idx, _)| idx < start || idx >= end);
                if value == self.default {
                    let mut rewritten = Vec::new();
                    for &(fstart, fend, fval) in fills.iter() {
                        if fend <= start || fstart >= end {
                            rewritten.push((fstart, fend, fval));
                        } else {
                            if fstart < start {
                                rewritten.push((fstart, start, fval));
                            }
                            if fend > end {
                                rewritten.push((end, fend, fval));
                            }
                        }
                    }
                    *fills = rewritten;
                } else {
                    overwrite_fills(fills, start, end, value);
                }
            }
        }
    }

    /// Snapshot entries in `start..start + count`; `None` on out of bounds.
    pub fn read_slice(&self, start: u64, count: u64) -> Option<Vec<Value>> {
        let end = start.checked_add(count)?;
        match &self.storage {
            TableStorage::Dense(vec) => {
                let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
                    return None;
                };
                if end > vec.len() {
                    return None;
                }
                Some(vec[start..end].to_vec())
            }
            TableStorage::Sparse { len, elems, fills } => {
                if end > u64::from(*len) {
                    return None;
                }
                // `end <= len <= u32::MAX`, so both narrowings are exact.
                let (start, end) = (start as u32, end as u32);
                let mut out = Vec::new();
                out.try_reserve(count as usize).ok()?;
                for idx in start..end {
                    out.push(
                        entry_get(elems, idx)
                            .or_else(|| fill_value(fills, idx))
                            .unwrap_or(self.default),
                    );
                }
                Some(out)
            }
        }
    }

    /// Write `values` starting at `start`; `false` on out of bounds.
    pub fn write_slice(&mut self, start: u64, values: &[Value]) -> bool {
        let Some(end) = start.checked_add(values.len() as u64) else {
            return false;
        };
        match &mut self.storage {
            TableStorage::Dense(vec) => {
                let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
                    return false;
                };
                if end > vec.len() {
                    return false;
                }
                vec[start..end].copy_from_slice(values);
            }
            TableStorage::Sparse { len, elems, fills } => {
                if end > u64::from(*len) {
                    return false;
                }
                // `end <= len <= u32::MAX`, so `start + offset` cannot wrap.
                let start = start as u32;
                for (offset, value) in values.iter().enumerate() {
                    let idx = start + offset as u32;
                    if *value == self.default {
                        entry_remove(elems, idx);
                        // Same unconditional hole rule as `set`.
                        if fill_value(fills, idx).is_some() {
                            entry_upsert(elems, idx, self.default);
                        }
                    } else {
                        entry_upsert(elems, idx, *value);
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

    fn funcref_table(min: u64, max: Option<u64>) -> TableType {
        TableType {
            elem: RefType::FuncRef,
            limits: Limits {
                min,
                max,
                is64: false,
            },
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
        let mut table = Table::new(
            funcref_table(u64::from(u32::MAX), None),
            Value::FuncRef(None),
        )
        .unwrap();
        assert_eq!(table.len(), u64::from(u32::MAX));
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
        let old = table
            .grow(u64::from(DENSE_LIMIT), Value::FuncRef(None))
            .unwrap();
        assert_eq!(old, 4);
        assert!(matches!(table.storage, TableStorage::Sparse { .. }));
        // Contents survive the migration.
        assert_eq!(table.get(2), Some(Value::FuncRef(Some((7, 7)))));
        assert_eq!(
            table.get(u64::from(DENSE_LIMIT)),
            Some(Value::FuncRef(None))
        );
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
