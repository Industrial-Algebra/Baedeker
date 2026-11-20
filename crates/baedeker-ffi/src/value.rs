// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! The C ABI value representation and conversions to/from `baedeker_core::Value`.
//!
//! Reference values never cross the boundary as raw heap pointers. `i31` is an
//! immediate and maps directly to an [`BaedekerValueTag::I31`] carrying its
//! `i32` payload; GC references (`GcRef`), internalized host references
//! (`HostAny`) and externalized GC references (`ExternGc`) map to **opaque
//! `u64` handle tokens** minted into a per-instance [`HandleRegistry`]. A token
//! is valid for the registry's (i.e. the instance store's) lifetime; there is
//! no release API — collector interaction is slice 5b.
//!
//! A token that was never minted by this registry (or was minted by a different
//! instance's registry) is a clean error, never a wild pointer dereference.

use baedeker_core::runtime::Value;
use baedeker_core::types::{NumType, ValType, VecType};
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-global token counter. Tokens are minted for the WHOLE process, not
/// per registry: two per-instance registries that both started at 1 would let
/// instance A's opaque handle decode — as an unrelated value — in instance
/// B's registry. `resolve` still only finds tokens its own registry minted;
/// global uniqueness makes a foreign token unambiguously unknown.
///
/// 1 is the first token and 0 is never valid. Reaching `u64::MAX` is checked
/// and reports [`HandleError::TokenExhausted`] instead of wrapping.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn mint_token() -> Result<u64, HandleError> {
    let mut current = NEXT_TOKEN.load(Ordering::SeqCst);
    loop {
        let next = current.checked_add(1).ok_or(HandleError::TokenExhausted)?;
        match NEXT_TOKEN.compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return Ok(current),
            Err(actual) => current = actual,
        }
    }
}

/// Type tag for [`BaedekerValue`], and the value-type encoding used when
/// declaring host function signatures.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaedekerValueTag {
    I32 = 0,
    I64 = 1,
    F32 = 2,
    F64 = 3,
    V128 = 4,
    /// A GC `i31` immediate (payload in `data.i32_`). Never a heap object.
    I31 = 5,
    /// A GC aggregate reference, carried as an opaque `u64` handle token in
    /// `data.handle`.
    GcRef = 6,
    /// An internalized host reference (`any.convert_extern`), carried as an
    /// opaque `u64` handle token in `data.handle`.
    HostAny = 7,
    /// An externalized GC reference (`extern.convert_any`), carried as an
    /// opaque `u64` handle token in `data.handle`.
    ExternGc = 8,
}

impl BaedekerValueTag {
    pub(crate) fn from_u8(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::I32,
            1 => Self::I64,
            2 => Self::F32,
            3 => Self::F64,
            4 => Self::V128,
            5 => Self::I31,
            6 => Self::GcRef,
            7 => Self::HostAny,
            8 => Self::ExternGc,
            _ => return None,
        })
    }

    pub(crate) fn val_type(self) -> ValType {
        match self {
            Self::I32 => ValType::Num(NumType::I32),
            Self::I64 => ValType::Num(NumType::I64),
            Self::F32 => ValType::Num(NumType::F32),
            Self::F64 => ValType::Num(NumType::F64),
            Self::V128 => ValType::Vec(VecType::V128),
            Self::I31 => ValType::Ref(baedeker_core::types::RefType::from_parts(
                false,
                baedeker_core::types::HeapType::I31,
            )),
            Self::GcRef => ValType::Ref(baedeker_core::types::RefType::from_parts(
                true,
                baedeker_core::types::HeapType::Any,
            )),
            Self::HostAny => ValType::Ref(baedeker_core::types::RefType::from_parts(
                true,
                baedeker_core::types::HeapType::Any,
            )),
            Self::ExternGc => ValType::Ref(baedeker_core::types::RefType::ExternRef),
        }
    }

    /// Whether this tag carries an opaque registry token rather than a value.
    pub fn is_handle(self) -> bool {
        matches!(self, Self::GcRef | Self::HostAny | Self::ExternGc)
    }
}

/// Payload of a [`BaedekerValue`]; the active field is selected by the tag.
/// Floats are carried as C `float`/`double` values, vectors as 16 raw bytes,
/// opaque handles as `u64` tokens.
#[repr(C)]
#[derive(Clone, Copy)]
pub union BaedekerValueData {
    pub i32_: i32,
    pub i64_: i64,
    pub f32_: f32,
    pub f64_: f64,
    pub v128: [u8; 16],
    /// An opaque handle token minted by a [`HandleRegistry`].
    pub handle: u64,
}

/// A WebAssembly value across the FFI boundary.
///
/// `tag` is a [`BaedekerValueTag`] value carried as a plain `uint8_t`:
/// fixed-underlying-type enums are C23-only, and a C-int enum here would
/// read 3 bytes of undefined struct padding against Rust's 1-byte write.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BaedekerValue {
    pub tag: u8,
    pub data: BaedekerValueData,
}

/// Why an opaque handle could not be converted back to a core value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleError {
    /// The token was never minted by this registry (or belongs to another
    /// instance's registry).
    UnknownToken(u64),
    /// The token resolves to a value of a different reference kind than the
    /// tag claims.
    KindMismatch {
        tag: BaedekerValueTag,
        found: BaedekerValueTag,
    },
    /// The registry could not reserve storage for a new token.
    AllocationFailed,
    /// The process-global token space is exhausted (`u64::MAX` reached); no
    /// further handles can be minted without reusing a token.
    TokenExhausted,
}

impl std::fmt::Display for HandleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownToken(token) => write!(f, "unknown GC handle token {token}"),
            Self::KindMismatch { tag, found } => {
                write!(f, "handle token is a {found:?}, not a {tag:?}")
            }
            Self::AllocationFailed => write!(f, "handle registry allocation refused"),
            Self::TokenExhausted => write!(f, "handle token space exhausted"),
        }
    }
}

impl std::error::Error for HandleError {}

/// A per-instance token → `Value` registry for opaque GC handles.
///
/// Tokens are minted from a process-global counter (`NEXT_TOKEN`), start
/// at 1 (0 is never a valid token), and are never reused, so a token stays
/// valid for the store's lifetime and a stale or foreign token resolves to
/// [`HandleError::UnknownToken`] instead of being misread as an unrelated
/// value. Minting is fallible: a refused reservation reports an error rather
/// than aborting, and an exhausted token space is a clean error too.
#[derive(Debug)]
pub struct HandleRegistry {
    values: Vec<(u64, Value)>,
}

impl Default for HandleRegistry {
    /// Same state as [`HandleRegistry::new`]: a derived `Default` would leave
    /// a zero counter behind and mint the invalid token 0 as its first token.
    fn default() -> Self {
        Self::new()
    }
}

impl HandleRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self { values: Vec::new() }
    }

    /// Mint an opaque token for `value`. Tokens are never released (no
    /// release API in this slice; the collector work owns reclamation).
    pub fn mint(&mut self, value: Value) -> Result<u64, HandleError> {
        // Reserve fallibly so a huge registry refuses cleanly instead of
        // aborting on allocator failure.
        self.values
            .try_reserve(1)
            .map_err(|_| HandleError::AllocationFailed)?;
        let token = mint_token()?;
        self.values.push((token, value));
        Ok(token)
    }

    /// Resolve a token back to its value, if this registry minted it.
    pub fn resolve(&self, token: u64) -> Option<Value> {
        self.values
            .iter()
            .find(|(minted, _)| *minted == token)
            .map(|(_, value)| *value)
    }

    /// Number of live handles.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether no handles have been minted.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl BaedekerValue {
    /// Convert a core value; `None` for reference values this conversion
    /// cannot express (funcref/externref/exnref/null references). GC values
    /// convert through [`BaedekerValue::from_core_with_handles`].
    pub(crate) fn from_core(value: Value) -> Option<Self> {
        Some(match value {
            Value::I32(v) => Self {
                tag: BaedekerValueTag::I32 as u8,
                data: BaedekerValueData { i32_: v },
            },
            Value::I64(v) => Self {
                tag: BaedekerValueTag::I64 as u8,
                data: BaedekerValueData { i64_: v },
            },
            Value::F32(v) => Self {
                tag: BaedekerValueTag::F32 as u8,
                data: BaedekerValueData { f32_: v },
            },
            Value::F64(v) => Self {
                tag: BaedekerValueTag::F64 as u8,
                data: BaedekerValueData { f64_: v },
            },
            Value::V128(bytes) => Self {
                tag: BaedekerValueTag::V128 as u8,
                data: BaedekerValueData { v128: bytes },
            },
            Value::I31(payload) => Self {
                tag: BaedekerValueTag::I31 as u8,
                data: BaedekerValueData { i32_: payload },
            },
            // Reference kinds this conversion cannot express: a null, a
            // funcref/externref/exnref, or a GC reference that needs a handle
            // registry (see `from_core_with_handles`).
            Value::FuncRef(_)
            | Value::ExternRef(_)
            | Value::ExnRef(_)
            | Value::GcRef { .. }
            | Value::HostAny(_)
            | Value::ExternGc(_) => return None,
        })
    }

    /// Convert a core value, minting opaque tokens for GC references.
    ///
    /// `i31` maps directly to its `i32` payload; `GcRef`, `HostAny` and
    /// `ExternGc` map to fresh tokens in `handles`. Null references and
    /// non-GC reference kinds are not representable and yield `None`.
    pub fn from_core_with_handles(
        value: Value,
        handles: &mut HandleRegistry,
    ) -> Option<Result<Self, HandleError>> {
        let tag = match value {
            Value::I31(payload) => {
                return Some(Ok(Self {
                    tag: BaedekerValueTag::I31 as u8,
                    data: BaedekerValueData { i32_: payload },
                }));
            }
            Value::GcRef { .. } => BaedekerValueTag::GcRef,
            Value::HostAny(Some(_)) => BaedekerValueTag::HostAny,
            Value::ExternGc(Some(_)) => BaedekerValueTag::ExternGc,
            _ => return Self::from_core(value).map(Ok),
        };
        Some(handles.mint(value).map(|token| Self {
            tag: tag as u8,
            data: BaedekerValueData { handle: token },
        }))
    }

    pub(crate) fn to_core(self) -> Value {
        unsafe {
            match self.tag {
                t if t == BaedekerValueTag::I32 as u8 => Value::I32(self.data.i32_),
                t if t == BaedekerValueTag::I64 as u8 => Value::I64(self.data.i64_),
                t if t == BaedekerValueTag::F32 as u8 => Value::F32(self.data.f32_),
                t if t == BaedekerValueTag::F64 as u8 => Value::F64(self.data.f64_),
                t if t == BaedekerValueTag::V128 as u8 => Value::V128(self.data.v128),
                t if t == BaedekerValueTag::I31 as u8 => Value::I31(self.data.i32_),
                _ => unreachable!("BaedekerValue tags are constructed from BaedekerValueTag"),
            }
        }
    }

    /// Convert back to a core value, resolving opaque handle tokens against
    /// `handles`. An unknown token — or one minted by another instance's
    /// registry — is a clean [`HandleError::UnknownToken`].
    pub fn to_core_with_handles(self, handles: &HandleRegistry) -> Result<Value, HandleError> {
        let tag = BaedekerValueTag::from_u8(self.tag)
            .ok_or(HandleError::UnknownToken(unsafe { self.data.handle }))?;
        if !tag.is_handle() {
            return Ok(self.to_core());
        }
        let token = unsafe { self.data.handle };
        let value = handles
            .resolve(token)
            .ok_or(HandleError::UnknownToken(token))?;
        let found = value_tag(value);
        if found != tag {
            return Err(HandleError::KindMismatch { tag, found });
        }
        Ok(value)
    }
}

/// The reference kind of a GC core value, as its FFI tag.
fn value_tag(value: Value) -> BaedekerValueTag {
    match value {
        Value::I31(_) => BaedekerValueTag::I31,
        Value::GcRef { .. } => BaedekerValueTag::GcRef,
        Value::HostAny(_) => BaedekerValueTag::HostAny,
        Value::ExternGc(_) => BaedekerValueTag::ExternGc,
        Value::I32(_) => BaedekerValueTag::I32,
        Value::I64(_) => BaedekerValueTag::I64,
        Value::F32(_) => BaedekerValueTag::F32,
        Value::F64(_) => BaedekerValueTag::F64,
        Value::V128(_) => BaedekerValueTag::V128,
        Value::FuncRef(_) | Value::ExternRef(_) | Value::ExnRef(_) => BaedekerValueTag::I32,
    }
}
