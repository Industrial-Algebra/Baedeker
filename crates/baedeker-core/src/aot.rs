// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! AOT artifact format: a postcard-serialized [`RegModule`] behind a magic
//! and format-version envelope.
//!
//! The AOT pipeline compiles a WASM binary to register IR at build time
//! ([`serialize`]), bundles the artifact into the host app, and deserializes
//! on device ([`deserialize`]) — skipping decode/validate/lower at startup.
//! Artifacts are **not** a stable cross-version format: the version byte must
//! match exactly, and consumers should rebuild artifacts with the runtime.
//!
//! Trust note: deserializing skips validation. Only load artifacts produced
//! by [`serialize`] from a validated module — treat artifacts like compiled
//! code, not like user input.

use alloc::vec::Vec;

use crate::lower::RegModule;

/// Artifact magic bytes (`"BDKAOT1"`).
pub const AOT_MAGIC: [u8; 7] = *b"BDKAOT1";
/// Envelope size: magic + u32 format version.
const ENVELOPE_LEN: usize = AOT_MAGIC.len() + 4;

/// Current artifact format version. Bump on any IR layout change.
pub const AOT_FORMAT_VERSION: u32 = 1;

/// Errors deserializing an AOT artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AotError {
    /// Fewer bytes than the envelope requires.
    TooShort,
    /// Magic bytes do not match.
    BadMagic,
    /// Format version differs from this runtime's [`AOT_FORMAT_VERSION`].
    UnsupportedVersion {
        /// The version found in the artifact.
        found: u32,
        /// The version this runtime understands.
        expected: u32,
    },
    /// The postcard payload did not decode into a `RegModule`.
    MalformedPayload,
}

impl core::fmt::Display for AotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort => write!(f, "artifact shorter than the envelope"),
            Self::BadMagic => write!(f, "artifact magic mismatch"),
            Self::UnsupportedVersion { found, expected } => write!(
                f,
                "artifact format version {found} (this runtime understands {expected})"
            ),
            Self::MalformedPayload => write!(f, "artifact payload failed to decode"),
        }
    }
}

impl core::error::Error for AotError {}

/// Serialize a lowered module into an AOT artifact.
pub fn serialize(module: &RegModule) -> Vec<u8> {
    let payload = postcard::to_allocvec(module).expect("RegModule serialization is infallible");
    let mut out = Vec::with_capacity(ENVELOPE_LEN + payload.len());
    out.extend_from_slice(&AOT_MAGIC);
    out.extend_from_slice(&AOT_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Deserialize an AOT artifact back into a `RegModule`.
///
/// The input may be freed after this returns — the module owns all its data.
pub fn deserialize(bytes: &[u8]) -> Result<RegModule, AotError> {
    if bytes.len() < ENVELOPE_LEN {
        return Err(AotError::TooShort);
    }
    if bytes[..AOT_MAGIC.len()] != AOT_MAGIC {
        return Err(AotError::BadMagic);
    }
    let version = u32::from_le_bytes(
        bytes[AOT_MAGIC.len()..ENVELOPE_LEN]
            .try_into()
            .expect("envelope slice length checked above"),
    );
    if version != AOT_FORMAT_VERSION {
        return Err(AotError::UnsupportedVersion {
            found: version,
            expected: AOT_FORMAT_VERSION,
        });
    }
    postcard::from_bytes(&bytes[ENVELOPE_LEN..]).map_err(|_| AotError::MalformedPayload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::module::Module;

    fn tiny_module() -> RegModule {
        let buf = wast::parser::ParseBuffer::new(
            "(module (func (export \"add\") (param i32 i32) (result i32)\n  local.get 0 local.get 1 i32.add))",
        )
        .unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        Module::decode(&bytes).unwrap().lower().unwrap()
    }

    #[test]
    fn roundtrip_preserves_module() {
        let module = tiny_module();
        let artifact = serialize(&module);
        assert_eq!(&artifact[..AOT_MAGIC.len()], &AOT_MAGIC);
        assert_eq!(deserialize(&artifact), Ok(module));
    }

    #[test]
    fn rejects_bad_envelope() {
        let module = tiny_module();
        let artifact = serialize(&module);

        assert_eq!(deserialize(&artifact[..4]), Err(AotError::TooShort));

        let mut bad_magic = artifact.clone();
        bad_magic[0] = b'X';
        assert_eq!(deserialize(&bad_magic), Err(AotError::BadMagic));

        let mut bad_version = artifact.clone();
        bad_version[AOT_MAGIC.len()] = 0xff;
        assert_eq!(
            deserialize(&bad_version),
            Err(AotError::UnsupportedVersion {
                found: 0xff,
                expected: AOT_FORMAT_VERSION,
            })
        );

        let truncated = &artifact[..artifact.len() - 3];
        assert_eq!(deserialize(truncated), Err(AotError::MalformedPayload));
    }
}
