// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Top-level WASM module decoding.
//!
//! Produces a `Module` — a parsed but not yet validated representation of a
//! WASM binary. At this phase, section contents are stored as raw byte spans.
//! Future phases will add full section content parsing.
//!
//! See [Spec §5.5.1](https://webassembly.github.io/spec/core/binary/modules.html).

use alloc::vec::Vec;

use crate::binary::codesec;
use crate::binary::datasec;
use crate::binary::elemsec;
use crate::binary::exportsec;
use crate::binary::functionsec;
use crate::binary::globalsec;
use crate::binary::importsec;
use crate::binary::leb128::Cursor;
use crate::binary::memorysec;
use crate::binary::section::{self, RawSection, SectionId};
use crate::binary::startsec;
use crate::binary::tablesec;
use crate::binary::typesec;
use crate::error::{ByteOffset, DecodeError, DecodeErrorKind};
use crate::types::{
    CodeBody, DataSegment, ElementSegment, Export, FuncIdx, FuncType, Global, Import, MemType,
    TableType, TypeIdx,
};

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
    /// Exported items declared by the module.
    pub exports: Vec<Export>,
    /// Type indices for module-defined (non-imported) functions.
    pub functions: Vec<TypeIdx>,
    /// Defined tables from the table section.
    pub tables: Vec<TableType>,
    /// Defined element segments from the element section.
    pub elements: Vec<ElementSegment<'a>>,
    /// Defined globals from the global section.
    pub globals: Vec<Global<'a>>,
    /// Defined memories from the memory section.
    pub memories: Vec<MemType>,
    /// Defined data segments from the data section.
    pub data: Vec<DataSegment<'a>>,
    /// Optional declared start function.
    pub start: Option<FuncIdx>,
    /// Optional declared data count.
    pub data_count: Option<u32>,
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
        let exports = match sections.iter().find(|s| s.id == SectionId::Export) {
            Some(section) => exportsec::parse_export_section(section)?,
            None => Vec::new(),
        };
        let functions = match sections.iter().find(|s| s.id == SectionId::Function) {
            Some(section) => functionsec::parse_function_section(section)?,
            None => Vec::new(),
        };
        let tables = match sections.iter().find(|s| s.id == SectionId::Table) {
            Some(section) => tablesec::parse_table_section(section)?,
            None => Vec::new(),
        };
        let elements = match sections.iter().find(|s| s.id == SectionId::Element) {
            Some(section) => elemsec::parse_element_section(section)?,
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
        let data = match sections.iter().find(|s| s.id == SectionId::Data) {
            Some(section) => datasec::parse_data_section(section)?,
            None => Vec::new(),
        };
        let start = match sections.iter().find(|s| s.id == SectionId::Start) {
            Some(section) => Some(startsec::parse_start_section(section)?),
            None => None,
        };
        let data_count = match sections.iter().find(|s| s.id == SectionId::DataCount) {
            Some(section) => Some(datasec::parse_data_count_section(section)?),
            None => None,
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
            exports,
            functions,
            tables,
            elements,
            globals,
            memories,
            data,
            start,
            data_count,
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

    /// Exported items declared by the module.
    pub fn exports(&self) -> &[Export] {
        &self.exports
    }

    /// Type indices for module-defined functions.
    pub fn functions(&self) -> &[TypeIdx] {
        &self.functions
    }

    /// Defined tables from the table section.
    pub fn tables(&self) -> &[TableType] {
        &self.tables
    }

    /// Defined element segments from the element section.
    pub fn elements(&self) -> &[ElementSegment<'a>] {
        &self.elements
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

    /// Defined data segments from the data section.
    pub fn data(&self) -> &[DataSegment<'a>] {
        &self.data
    }

    /// Declared start function, if present.
    pub fn start(&self) -> Option<FuncIdx> {
        self.start
    }

    /// Declared data count, if present.
    pub fn data_count(&self) -> Option<u32> {
        self.data_count
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
        assert!(module.exports.is_empty());
        assert!(module.functions.is_empty());
        assert!(module.tables.is_empty());
        assert!(module.elements.is_empty());
        assert!(module.globals.is_empty());
        assert!(module.memories.is_empty());
        assert!(module.data.is_empty());
        assert!(module.start.is_none());
        assert!(module.data_count.is_none());
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
        assert!(module.exports.is_empty());
        assert_eq!(module.functions, vec![TypeIdx(0)]);
        assert!(module.tables.is_empty());
        assert!(module.elements.is_empty());
        assert!(module.globals.is_empty());
        assert!(module.memories.is_empty());
        assert!(module.data.is_empty());
        assert!(module.start.is_none());
        assert!(module.data_count.is_none());
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
        assert!(module.exports().is_empty());
        assert!(module.functions().is_empty());
        assert!(module.tables().is_empty());
        assert!(module.elements().is_empty());
        assert!(module.globals().is_empty());
        assert!(module.memories().is_empty());
        assert!(module.data().is_empty());
        assert!(module.start().is_none());
        assert!(module.data_count().is_none());
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
    fn reject_gc_rec_type_group_boundary() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/invalid-decode/gc-rec-type-group.wasm");
        let err = Module::decode(bytes).unwrap_err();
        assert_eq!(err.offset, ByteOffset(11));
        assert_eq!(err.context, crate::error::DecodeContext::TypeSection);
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x60,
                found: 0x4E,
            }
        ));
    }

    #[test]
    fn reject_gc_sub_type_definition_boundary() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-decode/gc-sub-type-definition.wasm",
        );
        let err = Module::decode(bytes).unwrap_err();
        assert_eq!(err.offset, ByteOffset(11));
        assert_eq!(err.context, crate::error::DecodeContext::TypeSection);
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x60,
                found: 0x50,
            }
        ));
    }

    #[test]
    fn reject_gc_struct_type_definition_boundary() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-decode/gc-struct-type-definition.wasm",
        );
        let err = Module::decode(bytes).unwrap_err();
        assert_eq!(err.offset, ByteOffset(11));
        assert_eq!(err.context, crate::error::DecodeContext::TypeSection);
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x60,
                found: 0x5F,
            }
        ));
    }

    #[test]
    fn reject_gc_array_type_definition_boundary() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-decode/gc-array-type-definition.wasm",
        );
        let err = Module::decode(bytes).unwrap_err();
        assert_eq!(err.offset, ByteOffset(11));
        assert_eq!(err.context, crate::error::DecodeContext::TypeSection);
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x60,
                found: 0x5E,
            }
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
        assert!(
            !module.exports().is_empty(),
            "add.wasm export section should decode exports"
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
    fn decode_module_with_table_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x02,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.tables().len(), 1);
        assert_eq!(module.tables()[0].elem, crate::types::RefType::FuncRef);
        assert_eq!(module.tables()[0].limits.min, 2);
        assert_eq!(module.tables()[0].limits.max, None);
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

    #[test]
    fn decode_module_with_data_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0B,
            0x08, 0x01, 0x00, 0x41, 0x00, 0x0B, 0x02, 0xAA, 0xBB, 0x0C, 0x01, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.data().len(), 1);
        assert_eq!(module.data()[0].init, &[0xAA, 0xBB]);
        assert_eq!(module.data_count(), Some(1));
    }

    #[test]
    fn decode_module_with_export_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x07, 0x07, 0x01, 0x03, b'a', b'd',
            b'd', 0x00, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.exports().len(), 1);
        assert_eq!(module.exports()[0].name, "add");
        assert_eq!(
            module.exports()[0].desc,
            crate::types::ExportDesc::Func(crate::types::FuncIdx(0))
        );
    }

    #[test]
    fn decode_module_with_start_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x08, 0x01, 0x02,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.start(), Some(crate::types::FuncIdx(2)));
    }

    #[test]
    fn decode_module_with_element_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x09, 0x08, 0x01, 0x00, 0x41, 0x00,
            0x0B, 0x02, 0x00, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        assert_eq!(module.elements().len(), 1);
        assert_eq!(
            module.elements()[0].elem_type,
            crate::types::RefType::FuncRef
        );
        assert!(matches!(
            module.elements()[0].mode,
            crate::types::ElementMode::Active {
                table: crate::types::TableIdx(0),
                ..
            }
        ));
    }
}
