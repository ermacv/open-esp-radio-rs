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
