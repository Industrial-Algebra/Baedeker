//! Top-level WASM module decoding.
//!
//! Produces a `Module` — a parsed but not yet validated representation of a
//! WASM binary. At this phase, section contents are stored as raw byte spans.
//! Future phases will add full section content parsing.
//!
//! See [Spec §5.5.1](https://webassembly.github.io/spec/core/binary/modules.html).

use alloc::vec::Vec;

use crate::binary::codesec;
use crate::binary::functionsec;
use crate::binary::globalsec;
use crate::binary::importsec;
use crate::binary::leb128::Cursor;
use crate::binary::memorysec;
use crate::binary::section::{self, RawSection, SectionId};
use crate::binary::typesec;
use crate::error::{ByteOffset, DecodeError, DecodeErrorKind};
use crate::types::{CodeBody, FuncType, Global, Import, MemType, TypeIdx};

/// A parsed WASM module. Contains the raw section data segmented by type.
///
/// At Phase 0, section bodies are stored as raw bytes. Later phases will
/// parse type entries, imports, function bodies, etc.
#[derive(Debug)]
pub struct Module<'a> {
    /// All sections in the order they appeared, as raw byte spans.
    pub sections: Vec<RawSection<'a>>,
    /// Decoded function signatures from the type section.
    pub types: Vec<FuncType>,
    /// Imported items declared by the module.
    pub imports: Vec<Import>,
    /// Type indices for module-defined (non-imported) functions.
    pub functions: Vec<TypeIdx>,
    /// Defined globals from the global section.
    pub globals: Vec<Global<'a>>,
    /// Defined memories from the memory section.
    pub memories: Vec<MemType>,
    /// Function bodies from the code section.
    pub codes: Vec<CodeBody<'a>>,
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
        let types = match sections.iter().find(|s| s.id == SectionId::Type) {
            Some(section) => typesec::parse_type_section(section)?,
            None => Vec::new(),
        };
        let imports = match sections.iter().find(|s| s.id == SectionId::Import) {
            Some(section) => importsec::parse_import_section(section)?,
            None => Vec::new(),
        };
        let functions = match sections.iter().find(|s| s.id == SectionId::Function) {
            Some(section) => functionsec::parse_function_section(section)?,
            None => Vec::new(),
        };
        let globals = match sections.iter().find(|s| s.id == SectionId::Global) {
            Some(section) => globalsec::parse_global_section(section)?,
            None => Vec::new(),
        };
        let memories = match sections.iter().find(|s| s.id == SectionId::Memory) {
            Some(section) => memorysec::parse_memory_section(section)?,
            None => Vec::new(),
        };
        let codes = match sections.iter().find(|s| s.id == SectionId::Code) {
            Some(section) => codesec::parse_code_section(section)?,
            None => Vec::new(),
        };

        if functions.len() != codes.len() {
            return Err(DecodeError {
                offset: ByteOffset(0),
                context: crate::error::DecodeContext::CodeSection,
                kind: DecodeErrorKind::FunctionCodeLengthMismatch {
                    functions: functions.len() as u32,
                    codes: codes.len() as u32,
                },
            });
        }

        Ok(Module {
            sections,
            types,
            imports,
            functions,
            globals,
            memories,
            codes,
        })
    }

    /// Get the first section with the given ID, if present.
    pub fn section(&self, id: SectionId) -> Option<&RawSection<'a>> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// Function signatures declared in the type section.
    pub fn types(&self) -> &[FuncType] {
        &self.types
    }

    /// Imported items declared by the module.
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// Type indices for module-defined functions.
    pub fn functions(&self) -> &[TypeIdx] {
        &self.functions
    }

    /// Number of imported functions in the function index space.
    pub fn imported_function_count(&self) -> usize {
        self.imports
            .iter()
            .filter(|import| matches!(import.desc, crate::types::ImportDesc::Func(_)))
            .count()
    }

    /// Defined globals from the global section.
    pub fn globals(&self) -> &[Global<'a>] {
        &self.globals
    }

    /// Defined memories from the memory section.
    pub fn memories(&self) -> &[MemType] {
        &self.memories
    }

    /// Function bodies from the code section.
    pub fn codes(&self) -> &[CodeBody<'a>] {
        &self.codes
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
    use alloc::vec;

    use super::*;

    #[test]
    fn decode_minimal_module() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
        ];
        let module = Module::decode(&bytes).unwrap();
        assert!(module.sections.is_empty());
        assert!(module.types.is_empty());
        assert!(module.imports.is_empty());
        assert!(module.functions.is_empty());
        assert!(module.globals.is_empty());
        assert!(module.memories.is_empty());
        assert!(module.codes.is_empty());
    }

    #[test]
    fn decode_module_with_sections() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type section: 1 functype () -> ()
            0x03, 0x02, 0x01, 0x00, // function section: 1 function referencing type 0
            0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B, // code section: 1 empty body
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.sections.len(), 3);
        assert_eq!(module.sections[0].id, SectionId::Type);
        assert_eq!(module.sections[1].id, SectionId::Function);
        assert_eq!(module.sections[2].id, SectionId::Code);
        assert_eq!(module.types.len(), 1);
        assert!(module.types[0].params.is_empty());
        assert!(module.types[0].results.is_empty());
        assert_eq!(module.functions, vec![TypeIdx(0)]);
        assert!(module.globals.is_empty());
        assert!(module.memories.is_empty());
        assert_eq!(module.codes.len(), 1);
        assert!(module.codes[0].locals.is_empty());
        assert_eq!(module.codes[0].body, &[0x0B]);
    }

    #[test]
    fn section_lookup() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x01, 0x00, // type section: zero entries
        ];
        let module = Module::decode(&bytes).unwrap();
        assert!(module.section(SectionId::Type).is_some());
        assert!(module.section(SectionId::Import).is_none());
        assert_eq!(module.types().len(), 0);
        assert!(module.imports().is_empty());
        assert!(module.functions().is_empty());
        assert!(module.globals().is_empty());
        assert!(module.memories().is_empty());
        assert!(module.codes().is_empty());
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
        assert!(
            !module.types().is_empty(),
            "add.wasm should decode at least one function type"
        );
        assert!(
            !module.functions().is_empty(),
            "add.wasm should decode at least one function declaration"
        );
        assert!(
            !module.codes().is_empty(),
            "add.wasm should decode at least one function body"
        );
        assert_eq!(module.functions().len(), module.codes().len());

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
        if module.section(SectionId::Memory).is_some() {
            assert!(
                !module.memories().is_empty(),
                "memory.wasm memory section should decode memory types"
            );
        }
    }

    #[test]
    fn decode_module_with_global_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x06, 0x06, 0x01, 0x7F, 0x00, 0x41,
            0x2A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.globals().len(), 1);
        assert_eq!(module.globals()[0].init_expr, &[0x41, 0x2A, 0x0B]);
    }

    #[test]
    fn decode_module_with_memory_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x05, 0x03, 0x01, 0x00, 0x02,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.memories().len(), 1);
        assert_eq!(module.memories()[0].limits.min, 2);
        assert_eq!(module.memories()[0].limits.max, None);
    }
}
