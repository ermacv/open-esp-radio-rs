use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};

pub fn elf() -> Vec<u8> {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &[0x13, 0, 0, 0, 0x67, 0x80, 0, 0], 4);
    for (name, scope, weak) in [
        (b"same".to_vec(), SymbolScope::Linkage, false),
        (b"local\xff".to_vec(), SymbolScope::Compilation, false),
        (b"weak".to_vec(), SymbolScope::Linkage, true),
    ] {
        object.add_symbol(Symbol {
            name,
            value: 0,
            size: 8,
            kind: SymbolKind::Text,
            scope,
            weak,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
    }
    let external = object.add_symbol(Symbol {
        name: b"external".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Unknown,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object.add_common_symbol(
        Symbol {
            name: b"common".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Common,
            flags: SymbolFlags::None,
        },
        16,
        4,
    );
    object
        .add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: external,
                addend: 7,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

pub fn archive(members: &[(&[u8], &[u8])], thin: bool) -> Vec<u8> {
    let mut result = if thin {
        b"!<thin>\n".to_vec()
    } else {
        b"!<arch>\n".to_vec()
    };
    for (name, bytes) in members {
        assert!(name.len() < 16);
        let mut header = [b' '; 60];
        header[..name.len()].copy_from_slice(name);
        header[name.len()] = b'/';
        let fields = format!(
            "{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            0,
            0,
            0,
            "644",
            bytes.len()
        );
        header[16..].copy_from_slice(fields.as_bytes());
        result.extend_from_slice(&header);
        if !thin {
            result.extend_from_slice(bytes);
            if bytes.len() % 2 != 0 {
                result.push(b'\n');
            }
        }
    }
    result
}

/// Add a physical DYNSYM with reversed global entries. The name is deliberately
/// absent: table identity comes from ELF headers, never a conventional name.
#[allow(dead_code)]
pub fn dynamic_symbols(mut bytes: Vec<u8>, only: bool) -> Vec<u8> {
    fn word(b: &[u8], at: usize) -> usize {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
    }
    let shoff = word(&bytes, 32);
    let count = u16::from_le_bytes(bytes[48..50].try_into().unwrap()) as usize;
    assert_eq!(u16::from_le_bytes(bytes[46..48].try_into().unwrap()), 40);
    let mut headers = bytes[shoff..shoff + count * 40].to_vec();
    let static_index = (1..count)
        .find(|i| word(&headers, i * 40 + 4) == 2)
        .unwrap();
    let mut dynamic = headers[static_index * 40..(static_index + 1) * 40].to_vec();
    let offset = word(&dynamic, 16);
    let size = word(&dynamic, 20);
    assert_eq!(size % 16, 0);
    let locals = word(&dynamic, 28);
    let mut symbols = bytes[offset..offset + locals * 16].to_vec();
    for symbol in bytes[offset + locals * 16..offset + size]
        .chunks_exact(16)
        .rev()
    {
        symbols.extend_from_slice(symbol);
    }
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    dynamic[0..4].copy_from_slice(&0u32.to_le_bytes());
    dynamic[4..8].copy_from_slice(&11u32.to_le_bytes());
    dynamic[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&symbols);
    if only {
        headers[static_index * 40 + 4..static_index * 40 + 8].copy_from_slice(&1u32.to_le_bytes());
    }
    let new_shoff = bytes.len() as u32;
    bytes[32..36].copy_from_slice(&new_shoff.to_le_bytes());
    bytes[48..50].copy_from_slice(&((count + 1) as u16).to_le_bytes());
    bytes.extend_from_slice(&headers);
    bytes.extend_from_slice(&dynamic);
    bytes
}
