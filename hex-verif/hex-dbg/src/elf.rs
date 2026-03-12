// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! ELF symbol table reader.
//!
//! Resolves symbol names (e.g. `"steps"`, `"test_end"`) to virtual addresses
//! from a compiled Hexagon ELF binary.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use object::{Object, ObjectSymbol};

/// Read all named symbols from an ELF file.
///
/// Returns a map from symbol name to virtual address. Only symbols with
/// non-empty names and defined addresses are included.
pub fn read_symbols(elf_path: &Path) -> Result<HashMap<String, u64>> {
    let data = std::fs::read(elf_path)
        .with_context(|| format!("failed to read ELF: {}", elf_path.display()))?;
    let obj = object::File::parse(&*data)
        .with_context(|| format!("failed to parse ELF: {}", elf_path.display()))?;

    let mut symbols = HashMap::new();
    for sym in obj.symbols() {
        if let Ok(name) = sym.name() {
            if !name.is_empty() && sym.address() != 0 {
                symbols.insert(name.to_string(), sym.address());
            }
        }
    }

    Ok(symbols)
}

/// Resolve specific symbol names to their addresses.
///
/// Returns an error if any of the requested `names` is missing from the ELF.
pub fn resolve_symbols(elf_path: &Path, names: &[&str]) -> Result<Vec<(String, u64)>> {
    let all = read_symbols(elf_path)?;
    let mut result = Vec::with_capacity(names.len());
    for &name in names {
        match all.get(name) {
            Some(&addr) => result.push((name.to_string(), addr)),
            None => bail!("symbol '{}' not found in {}", name, elf_path.display()),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::write::{Object as WriteObject, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope};
    use std::io::Write;

    /// Create a minimal ELF in a temp file with the given symbols.
    fn make_test_elf(symbols: &[(&str, u64)]) -> tempfile::NamedTempFile {
        let mut obj =
            WriteObject::new(BinaryFormat::Elf, Architecture::Hexagon, Endianness::Little);

        let text_id = obj.section_id(object::write::StandardSection::Text);

        for &(name, addr) in symbols {
            obj.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: addr,
                size: 4,
                kind: SymbolKind::Text,
                scope: SymbolScope::Dynamic,
                weak: false,
                section: SymbolSection::Section(text_id),
                flags: SymbolFlags::None,
            });
        }

        let data = obj.write().expect("failed to write test ELF");
        let mut tmp = tempfile::NamedTempFile::new().expect("failed to create tempfile");
        tmp.write_all(&data).expect("failed to write tempfile");
        tmp
    }

    #[test]
    fn test_read_symbols() {
        let elf = make_test_elf(&[("steps", 0x1000), ("test_end", 0x2000)]);
        let syms = read_symbols(elf.path()).unwrap();
        assert_eq!(syms.get("steps"), Some(&0x1000));
        assert_eq!(syms.get("test_end"), Some(&0x2000));
    }

    #[test]
    fn test_resolve_symbols() {
        let elf = make_test_elf(&[("steps", 0x1000), ("test_end", 0x2000)]);
        let resolved = resolve_symbols(elf.path(), &["steps", "test_end"]).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0], ("steps".into(), 0x1000));
        assert_eq!(resolved[1], ("test_end".into(), 0x2000));
    }

    #[test]
    fn test_resolve_missing_symbol() {
        let elf = make_test_elf(&[("steps", 0x1000)]);
        let result = resolve_symbols(elf.path(), &["steps", "nonexistent"]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
    }
}
