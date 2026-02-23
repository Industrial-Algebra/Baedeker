//! Top-level WASM module decoding.
//!
//! Produces a `Module` — a parsed but not yet validated representation of a
//! WASM binary. At this phase, section contents are stored as raw byte spans.
//! Future phases will add full section content parsing.
//!
//! See [Spec §5.5.1](https://webassembly.github.io/spec/core/binary/modules.html).

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::binary::leb128::Cursor;
use crate::binary::section::{self, RawSection, SectionId};
use crate::error::DecodeError;

/// A parsed WASM module. Contains the raw section data segmented by type.
///
/// At Phase 0, section bodies are stored as raw bytes. Later phases will
/// parse type entries, imports, function bodies, etc.
#[derive(Debug)]
pub struct Module<'a> {
    /// All sections in the order they appeared, as raw byte spans.
    pub sections: Vec<RawSection<'a>>,
}

impl<'a> Module<'a> {
    /// Decode a WASM binary into a `Module`.
    ///
    /// This validates the preamble (magic + version), parses section boundaries,
    /// and checks section ordering. It does NOT validate section contents.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        let mut cursor = Cursor::new(bytes);

        section::parse_preamble(&mut cursor)?;
        let sections = section::parse_sections(&mut cursor)?;

        Ok(Module { sections })
    }

    /// Get the first section with the given ID, if present.
    pub fn section(&self, id: SectionId) -> Option<&RawSection<'a>> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// Iterate over all custom sections.
    pub fn custom_sections(&self) -> impl Iterator<Item = &RawSection<'a>> {
        self.sections.iter().filter(|s| s.id == SectionId::Custom)
    }

    /// Summary of the module's section layout for display.
    pub fn section_summary(&self) -> Vec<(SectionId, usize, usize)> {
        self.sections
            .iter()
            .map(|s| (s.id, s.offset, s.data.len()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_minimal_module() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
        ];
        let module = Module::decode(&bytes).unwrap();
        assert!(module.sections.is_empty());
    }

    #[test]
    fn decode_module_with_sections() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type section: 1 functype () -> ()
            0x03, 0x02, 0x01, 0x00, // function section: 1 function referencing type 0
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.sections.len(), 2);
        assert_eq!(module.sections[0].id, SectionId::Type);
        assert_eq!(module.sections[1].id, SectionId::Function);
    }

    #[test]
    fn section_lookup() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x01, 0xFF, // type section (1 byte)
        ];
        let module = Module::decode(&bytes).unwrap();
        assert!(module.section(SectionId::Type).is_some());
        assert!(module.section(SectionId::Import).is_none());
    }

    #[test]
    fn reject_truncated_binary() {
        let bytes = [0x00, 0x61]; // truncated magic
        let err = Module::decode(&bytes).unwrap_err();
        assert!(matches!(
            err.kind,
            crate::error::DecodeErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn reject_bad_magic() {
        let bytes = [
            0xDE, 0xAD, 0xBE, 0xEF, // wrong magic
            0x01, 0x00, 0x00, 0x00, // version
        ];
        let err = Module::decode(&bytes).unwrap_err();
        assert!(matches!(
            err.kind,
            crate::error::DecodeErrorKind::InvalidMagic
        ));
    }

    #[test]
    fn decode_empty_fixture() {
        let bytes = baedeker_testdata::fixture_bytes("empty");
        let module = Module::decode(&bytes).unwrap();
        // Minimal module has at least a valid preamble; may have custom sections
        // from the Rust toolchain but no required non-custom sections.
        for s in &module.sections {
            // Should parse without error regardless of section content.
            let _ = s.id.name();
        }
    }

    #[test]
    fn decode_add_fixture() {
        let bytes = baedeker_testdata::fixture_bytes("add");
        let module = Module::decode(&bytes).unwrap();

        // A cdylib with an exported function must have type, function, and code sections.
        assert!(
            module.section(SectionId::Type).is_some(),
            "add.wasm missing type section"
        );
        assert!(
            module.section(SectionId::Function).is_some(),
            "add.wasm missing function section"
        );
        assert!(
            module.section(SectionId::Code).is_some(),
            "add.wasm missing code section"
        );
        assert!(
            module.section(SectionId::Export).is_some(),
            "add.wasm missing export section"
        );
    }

    #[test]
    fn decode_memory_fixture() {
        let bytes = baedeker_testdata::fixture_bytes("memory");
        let module = Module::decode(&bytes).unwrap();

        // Module with static memory should have a memory or data section.
        assert!(
            module.section(SectionId::Memory).is_some()
                || module.section(SectionId::Data).is_some(),
            "memory.wasm missing memory/data section"
        );
    }
}
