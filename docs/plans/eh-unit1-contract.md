# EH Unit 1 Contract — binary + types surface

Unit: U1 of the EH ladder (`docs/plans/eh-slice.md`). Depends on: none.
Followed by: U2 (validation).

## Files

Create:
- `crates/baedeker-core/src/binary/tagsec.rs`
- `crates/baedeker-core/tests/eh_binary.rs`

Modify:
- `crates/baedeker-core/src/types.rs`
- `crates/baedeker-core/src/error.rs` (DecodeErrorKind variant)
- `crates/baedeker-core/src/binary/typeparser.rs`
- `crates/baedeker-core/src/binary/section.rs`
- `crates/baedeker-core/src/binary/module.rs`
- `crates/baedeker-core/src/binary/importsec.rs`
- `crates/baedeker-core/src/binary/exportsec.rs`
- `crates/baedeker-core/src/binary/instr.rs`
- `crates/baedeker-core/src/binary/mod.rs` (module declaration for tagsec)
- `crates/baedeker-core/src/validate/mod.rs` (match exhaustiveness only — new
  Instr/ImportDesc/ExportDesc arms go through the **not-yet-supported
  validation error** used for other post-2.0 features, so validation rejects
  EH modules with a clear error until U2; find the existing pattern for
  unimplemented-feature arms and mirror it)

## Contracts

### 1. `types.rs`

(a) Extend `HeapType` (currently `Func`, `Extern`, `Type(TypeIdx)`) with:

```rust
pub enum HeapType {
    Func,
    Extern,
    /// The `exn` abstract heap type (exception references).
    /// Part of the exception-handling proposal (wasm 3.0).
    Exn,
    Type(TypeIdx),
}
```

In `HeapType::is_subtype_of`, add the reflexive arm:

```rust
(HeapType::Exn, HeapType::Exn) => true,
```

(b) In `RefType::from_parts`, map the new variant (before the existing
`Type(type_idx)` arm style — note `Exn` has no shorthand variant):

```rust
HeapType::Exn => Self::Typed { nullable, heap: HeapType::Exn },
```

(c) Add a `TagIdx` newtype next to the existing `TypeIdx`/`FuncIdx` style
definitions (match the exact derive/def style used by `TypeIdx`):

```rust
/// Index of a tag in the module's tag index space.
pub type TagIdx = crate::types::Idx<TagMarker>;
```

NOTE: this shape is a placeholder — if `TypeIdx` is a plain tuple struct
(e.g. `pub struct TypeIdx(pub u32);`) then define `TagIdx` the same way:

```rust
pub struct TagIdx(pub u32);
```

with identical derives. **Use whatever pattern `TypeIdx` actually uses.**

### 2. `error.rs`

Add to `DecodeErrorKind` (match existing variant style/derives):

```rust
/// Tag section entry had a non-zero attribute byte.
InvalidTagAttribute { byte: u8 },
```

### 3. `typeparser.rs`

(a) In the valtype first-byte match (`parse_val_type_with_first_byte`),
add `0x69` to the accepted ref shorthand bytes and route it through
`parse_ref_type_with_first_byte`.

(b) In `parse_ref_type_with_first_byte`, add:

```rust
0x69 => Ok(RefType::from_parts(true, HeapType::Exn)),
```

(c) In `parse_heap_type_at`, remove `0x69` from the rejected-bytes list
(`0x6E | 0x71 | ... | 0x69 | ...`) and add an accept arm mirroring the
`0x70 => Func` one:

```rust
0x69 => {
    cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(absolute_offset),
        context,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    Ok(HeapType::Exn)
}
```

### 4. `section.rs`

(a) Add the variant to `SectionId` (after `DataCount`):

```rust
/// The tag section (exception handling, wasm 3.0).
Tag = 13,
```

Extend `from_byte` with `13 => Some(SectionId::Tag)` and `name` with
`SectionId::Tag => "tag"`.

(b) `section_order` currently returns the section id as its sort key. Tag's
binary position is between Memory and Global, so replace the body of
`section_order` with an explicit precedence table (ids keep their encoded
values; only the ORDER mapping changes):

```rust
fn section_order(id: SectionId) -> u8 {
    match id {
        SectionId::Type => 1,
        SectionId::Import => 2,
        SectionId::Function => 3,
        SectionId::Table => 4,
        SectionId::Memory => 5,
        SectionId::Tag => 6,
        SectionId::Global => 7,
        SectionId::Export => 8,
        SectionId::Start => 9,
        SectionId::Element => 10,
        SectionId::DataCount => 11,
        SectionId::Code => 12,
        SectionId::Data => 13,
    }
}
```

If the current implementation differs (e.g. derives order from the enum),
rewrite it to this exact table. Preserve `sections_in_valid_order` and its
special-case tuples unchanged (they compare these order values now).

### 5. `tagsec.rs` (new file)

Full content:

```rust
// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Tag section parsing.
//!
//! Decodes exception tag definitions. A tag entry is an attribute byte
//! (must be zero — only exception tags exist in wasm 3.0) followed by a
//! type index whose function type must have empty results (checked at
//! validation, not here).
//! See the exception-handling proposal and
//! [Spec §5.5.X](https://webassembly.github.io/exception-handling/core/binary/modules.html#tag-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::TypeIdx;

pub fn parse_tag_section(section: &RawSection<'_>) -> Result<Vec<TypeIdx>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut tags = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let pos = cursor.position();
        let attribute = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(section.offset + pos),
            context: DecodeContext::SectionBody { id: 13 },
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if attribute != 0x00 {
            return Err(DecodeError {
                offset: ByteOffset(section.offset + pos),
                context: DecodeContext::SectionBody { id: 13 },
                kind: DecodeErrorKind::InvalidTagAttribute { byte: attribute },
            });
        }
        tags.push(TypeIdx(decode_u32_in_section(
            &mut cursor,
            section.offset,
        )?));
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::SectionBody { id: 13 },
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(tags)
}
```

Adapt helper usage to the local idiom: if `globalsec.rs` has a private
`decode_u32_in_section` helper (it does — reuse the same pattern, either
duplicate the tiny helper or import it if shared), match exactly how
`globalsec.rs` decodes u32s and errors. The CONTEXT used for errors must
match what the error-type permits (`DecodeContext::SectionBody { id }`
or the section-specific context variants — mirror globalsec's choice,
substituting the tag section id).

### 6. `module.rs`

Add to `Module<'a>` (after `memories`):

```rust
/// Type indices for module-defined (non-imported) tags.
pub tags: Vec<TypeIdx>,
```

In the parse function, mirror the memory-section block:

```rust
let tags = match sections.iter().find(|s| s.id == SectionId::Tag) {
    Some(section) => tagsec::parse_tag_section(section)?,
    None => Vec::new(),
};
```

Wire `tags` into the struct literal. Add `use crate::binary::tagsec;` (or the
module-path idiom used for other section modules).

### 7. `importsec.rs`

`ImportDesc` gains (in `types.rs`, next to the other variants — appended
LAST to preserve any derived serialization stability):

```rust
/// Imported exception tag; the index is the tag's function type.
Tag(TypeIdx),
```

In the desc-byte match (currently `0x00 => Func`, `0x01 => Table`,
`0x02 => Mem`, `0x03 => Global`), add:

```rust
0x04 => {
    let pos = cursor.position();
    let attribute = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + pos),
        context: <same context as sibling arms>,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    if attribute != 0x00 {
        return Err(DecodeError {
            offset: ByteOffset(base_offset + pos),
            context: <same context as sibling arms>,
            kind: DecodeErrorKind::InvalidTagAttribute { byte: attribute },
        });
    }
    let type_idx = <decode u32 like the Func arm does>;
    Ok(ImportDesc::Tag(TypeIdx(type_idx)))
}
```

Match the surrounding error-construction idiom exactly.

### 8. `exportsec.rs`

`ExportDesc` gains (appended LAST):

```rust
/// Exported exception tag.
Tag(TagIdx),
```

Desc-byte match gains `0x04 => Ok(ExportDesc::Tag(TagIdx(<u32 decode like
Func arm>)))` mirroring the sibling arms.

### 9. `instr.rs`

(a) New types (place near `BlockType`/other instruction-support types):

```rust
/// A catch clause of a `try_table` instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct TryTableCatch {
    /// Which form of catch this clause is.
    pub kind: TryTableCatchKind,
    /// Tag index; `None` for `catch_all` / `catch_all_ref`.
    pub tag: Option<TagIdx>,
    /// Branch target label, relative like a `br` label.
    pub label: u32,
}

/// The four catch-clause forms of `try_table`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryTableCatchKind {
    /// `catch $tag $label` — match tag, branch with payload.
    Catch,
    /// `catch_ref $tag $label` — match tag, branch with payload + exnref.
    CatchRef,
    /// `catch_all $label` — match anything, branch empty.
    CatchAll,
    /// `catch_all_ref $label` — match anything, branch with exnref.
    CatchAllRef,
}
```

(Adjust derives to match sibling instruction types — check what `Instr`
itself derives and match.)

(b) `Instr` variants (appended at the END of the enum):

```rust
/// `try_table` — protected block with catch clauses (wasm 3.0 EH).
TryTable(TryTable),
/// `throw $tag` — raise an exception with the tag's parameters.
Throw(TagIdx),
/// `throw_ref` — re-raise the exception referenced by an exnref.
ThrowRef,
```

with

```rust
/// The `try_table` instruction payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TryTable {
    pub block_type: BlockType,
    pub catches: Vec<TryTableCatch>,
}
```

(c) Decode arms (in the single-byte opcode match, near `0x02 => Block`):

```rust
0x1F => Instr::TryTable(parse_try_table(cursor, base_offset)?),
0x08 => Instr::Throw(parse_tag_idx(cursor, base_offset)?),
0x0A => Instr::ThrowRef,
```

with helpers:

```rust
fn parse_tag_idx(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TagIdx, DecodeError> {
    let pos = cursor.position();
    let idx = leb128::decode_u32(cursor).map_err(|mut e| {
        e.offset = ByteOffset(base_offset + pos);
        e
    })?;
    Ok(TagIdx(idx))
}
```

(Adapt to the actual decode_u32 error idiom used for other immediate
helpers like `parse_lane_idx` — that function is the model: read the byte
or u32 with offset correction.)

```rust
fn parse_try_table(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TryTable, DecodeError> {
    let block_type = parse_block_type(cursor, base_offset)?;
    let pos = cursor.position();
    let count = leb128::decode_u32(cursor).map_err(|mut e| {
        e.offset = ByteOffset(base_offset + pos);
        e
    })?;
    let mut catches = Vec::new();
    for _ in 0..count {
        let clause_pos = cursor.position();
        let kind_byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + clause_pos),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        let (kind, has_tag) = match kind_byte {
            0x00 => (TryTableCatchKind::Catch, true),
            0x01 => (TryTableCatchKind::CatchRef, true),
            0x02 => (TryTableCatchKind::CatchAll, false),
            0x03 => (TryTableCatchKind::CatchAllRef, false),
            byte => {
                return Err(DecodeError {
                    offset: ByteOffset(base_offset + clause_pos),
                    context: DecodeContext::CodeSection,
                    kind: DecodeErrorKind::UnknownOpcode { byte },
                });
            }
        };
        let tag = if has_tag {
            Some(parse_tag_idx(cursor, base_offset)?)
        } else {
            None
        };
        let label_pos = cursor.position();
        let label = leb128::decode_u32(cursor).map_err(|mut e| {
            e.offset = ByteOffset(base_offset + label_pos);
            e
        })?;
        catches.push(TryTableCatch { kind, tag, label });
    }
    Ok(TryTable { block_type, catches })
}
```

(If an existing immediate-count loop idiom exists — e.g. br_table's label
vector — mirror its error style and use `Vec::with_capacity` via the
cursor's capacity hint if that idiom exists.)

(d) Block-depth tracking: the line
`Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => block_depth += 1` must
become
`Instr::Block(_) | Instr::Loop(_) | Instr::If(_) | Instr::TryTable(_) => block_depth += 1`.
Grep for other structural match sites over `Instr` that enumerate
Block/Loop/If (there may be several: end-matching, else-handling, nesting
audits) and extend each consistently — **report every site you touch in
the ambiguity footer**.

### 10. `validate/mod.rs`

Add match arms for the new variants that produce the module's standard
"not supported / not yet implemented" validation error — find how the
codebase rejects other future-feature instructions (or add a new
`ValidationErrorKind::Unsupported` style variant if none exists — report
which you chose). Same for `ImportDesc::Tag` / `ExportDesc::Tag` arms in
any exhaustive match over those enums (grep both). These are placeholder
arms U2 replaces; they must still be *validation* errors, not decode
errors.

## Tests (`tests/eh_binary.rs`, new integration test file)

License header + doc comment like `tests/simd_integer_lanes.rs`. Build
helper: a small `decode(bytes) -> Result<Module, _>` wrapper calling the
same entry the other integration tests use (`Module::parse` or
equivalent — check `tests/spec.rs` or `tests/aot_roundtrip.rs` for the
module decode entry point and mirror it).

Every test asserts BOTH success shapes and error kinds. Hand-built byte
vectors (each computed twice author-side):

1. `tag_section_decodes` — module: type section `(func (param i32))` +
   tag section with one entry (attr 0x00, typeidx 0). Bytes:
   `00 61 73 6D 01 00 00 00 | 01 05 01 60 01 7F 00 | 0D 03 01 00 00`
   Assert `module.tags == vec![TypeIdx(0)]`.
2. `tag_section_rejects_nonzero_attribute` — same but entry attr `0x01`
   (`0D 03 01 01 00`). Assert `DecodeErrorKind::InvalidTagAttribute { byte: 0x01 }`.
3. `tag_section_order_enforced` — tag section placed AFTER the global
   section (build: type `()`, global section with one i32 const global,
   then tag section). Assert the section-order decode error (same kind
   the existing order tests use — find one in section.rs tests and match).
4. `exnref_valtype_parses` — unit test at the typeparser level (mirror
   typeparser.rs's existing cursor tests): `[0x69]` parses to
   `ValType::Ref(RefType::Typed { nullable: true, heap: HeapType::Exn })`;
   `[0x63, 0x69]` parses to the same type; `[0x64, 0x69]` parses to
   `nullable: false`.
5. `throw_and_try_table_decode` — at the instr level (mirror instr.rs's
   decode tests around line 1445 which decode body bytes directly):
   - `0x08 0x00` ⇒ `Instr::Throw(TagIdx(0))`
   - `0x0A` ⇒ `Instr::ThrowRef`
   - `0x1F 0x40 0x02 0x00 0x00 0x01 0x03 0x02` ⇒ `TryTable` with
     blocktype Empty and two catches: `{Catch, tag: Some(TagIdx(0)), label: 1}`
     then `{CatchAllRef, tag: None, label: 2}`
   - bad clause kind `0x1F 0x40 0x01 0x04 0x00 0x00` ⇒ `UnknownOpcode { byte: 0x04 }`
6. `tag_import_export_decode` — module importing a tag
   (`(import "" "" (tag (type 0)))`) and exporting tag 0. Import bytes:
   import section with module/name len 0 each, desc `0x04`, attr `0x00`,
   typeidx 0. Export: name `"t"` desc `0x04` idx `0`. Assert
   `ImportDesc::Tag(TypeIdx(0))` and `ExportDesc::Tag(TagIdx(0))`.
7. `legacy_eh_opcodes_stay_unknown` — instr-level: `0x06`, `0x07`,
   `0x09`, `0x18`, `0x19` each decode-fail with `UnknownOpcode`.
8. `validation_rejects_eh_for_now` — full module with a tag + a function
   body `throw 0` decodes OK but FAILS validation with the placeholder
   error from Contract 10 (assert the specific error kind chosen).

## Constraints

- No new dependencies. `no_std` + alloc only. License headers on new files.
- Do not touch `lower/`, `runtime/`, or `aot.rs` (U3's territory).
- Only the files listed. Do not reformat unrelated code.
- serde-derived enums (`ImportDesc`, `ExportDesc`, `Instr` if derived):
  new variants appended LAST only.

## Completion

```
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|error' | tail -25
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
```

Both green, `eh_binary` tests all passing, and the pre-existing suites
unchanged (official summary line still `failures=0` in
`runtime_official` — the placeholder validation must not change how any
existing module validates).

## Out of scope

Lowering, execution, validation typing (U2+), tag identity/linking (U4),
legacy EH opcodes, AOT version bump.
