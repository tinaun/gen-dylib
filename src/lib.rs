use byteorder::{NativeEndian, BigEndian, WriteBytesExt};
use std::io::{self, Write};

use indexmap::IndexMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Import {
    Name(String),
    Ordinal(u16),
}

impl Import {
    fn name(&self) -> Option<&str> {
        match self {
            Import::Name(s) => Some(s),
            Import::Ordinal(_) => None,
        }
    }

    fn ordinal(&self) -> Option<u16> {
        match self {
            Import::Name(_) => None,
            Import::Ordinal(o) => Some(*o),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportLibBuilder {
    name: String,
    imports: Vec<(String, Import)>,
}

impl ImportLibBuilder {
    pub fn new(lib_name: &str) -> Self {
        Self {
            name: lib_name.to_string(),
            imports: vec![],
        }
    }

    pub fn import_name(self, rust_name: &str, lib_name: &str) -> Self {
        let mut this = self;
        this.imports.push((rust_name.to_owned(), Import::Name(lib_name.to_owned())));
        this
    }

    pub fn import_ordinal(self, rust_name: &str, lib_ordinal: u16) -> Self {
        let mut this = self;
        this.imports.push((rust_name.to_owned(), Import::Ordinal(lib_ordinal)));
        this
    }

    pub fn build(self) -> Vec<u8> {
        build_library(self).unwrap()
    }
}

const IMAGE_SYM_CLASS_EXTERNAL: u8 = 0;
const IMAGE_SYM_CLASS_SECTION: u8 = 0;
const IMAGE_SYM_CLASS_STATIC: u8 = 0;

const COFF_HEADER_LEN: usize = 0x14;
const COFF_SECTION_HEADER_LEN: usize = 0x12;
const ARCHIVE_HEADER_LEN: usize = 0x3C;
const ARCHIVE_SIG: &[u8] = b"!<arch>\n";


fn build_library(imports: ImportLibBuilder) -> io::Result<Vec<u8>> {
    let mut import_lib = ARCHIVE_SIG.to_vec();
    let mut archive_builder = CoffArchiveBuilder::new(&imports.name);

    archive_builder.add_import_descriptors()?;


    for (name, import) in &imports.imports {
        archive_builder.add_short_import(name, import)?;
    }

    println!("{:?}", archive_builder.symbols);

    let members = archive_builder.sections.len();
    let symbols: Vec<_> = archive_builder.symbols.into_iter().collect();

    let symbol_table_len = symbols.iter().map(|s| s.0.len() + 1).sum::<usize>();

    let first_linker_len = 4 + 4 * symbols.len() + symbol_table_len;
    let second_linker_len = 8 + 4 * members + 2 * symbols.len() + symbol_table_len;

    let mut import_start = ARCHIVE_SIG.len();
    import_start += ARCHIVE_HEADER_LEN + first_linker_len;
    if import_start % 2 != 0 {
        import_start += 1;
    }
    import_start += ARCHIVE_HEADER_LEN + second_linker_len;
    if import_start % 2 != 0 {
        import_start += 1;
    }

    let mut offsets = vec![];
    for d in &archive_builder.sections {
        offsets.push(import_start);
        import_start += ARCHIVE_HEADER_LEN + d.len();
        if import_start % 2 != 0 {
            import_start += 1;
        }
    }

    println!("{:?}, {:?}", offsets, symbols);
    write_header(&mut import_lib, "", first_linker_len)?;
    import_lib.write_u32::<BigEndian>(symbols.len() as u32)?; // number of symbols

    for (_name, i) in &symbols {
        let offset = offsets[i-1];
        import_lib.write_u32::<BigEndian>(offset as u32)?;
    }

    for symbol in &symbols {
        import_lib.write_all(symbol.0.as_bytes())?;
        import_lib.write_u8(b'\0')?;
    }

    if import_lib.len() % 2 != 0 {
        import_lib.write_u8(b'\0')?;
    }

    let mut symbols = symbols;
    symbols.sort_by_key(|c| c.1);

    write_header(&mut import_lib, "", second_linker_len)?;
    import_lib.write_u32::<NativeEndian>(members as u32)?;

    for offset in offsets {
        import_lib.write_u32::<NativeEndian>(offset as u32)?;
    }
    
    import_lib.write_u32::<NativeEndian>(symbols.len() as u32)?;
    for (_symbol, offset) in &symbols {
        import_lib.write_u16::<NativeEndian>(*offset as u16)?;
    }

    for symbol in &symbols {
        import_lib.write_all(symbol.0.as_bytes())?;
        import_lib.write_u8(b'\0')?;
    }

    if import_lib.len() % 2 != 0 {
        import_lib.write_u8(b'\0')?;
    }

    for data in archive_builder.sections {
        write_header(&mut import_lib, &imports.name, data.len())?;
        import_lib.write_all(&data)?;
        if import_lib.len() % 2 != 0 {
            import_lib.write_u8(b'\0')?;
        }
    }

    Ok(import_lib)
}

fn write_header<W: Write>(buf: &mut W, name: &str, len: usize) -> io::Result<()> {
    let name = format!("{}/", if name.len() > 15 {
        &name[0..15]
    } else {
        &name
    });

    write!(buf, "{:<16}", name)?;
    write!(buf, "{:<12}", -1)?; // Date (-1 in windows tools)
    write!(buf, "      ")?; // user id (all blanks)
    write!(buf, "      ")?; // group id (all blanks)
    write!(buf, "{:<8}", 0)?; // mode
    write!(buf, "{:<10}`\n", len)?; // size and end
    Ok(())
}

fn arch() -> u16 {
    if cfg!(target_arch = "x86_64") {
        0x8664
    } else if cfg!(target_arch = "x86") {
        0x014C
    } else {
        panic!("unsupported arch")
    }
}

#[derive(Debug)]
struct CoffArchiveBuilder {
    symbols: IndexMap<String, usize>,
    sections: Vec<Vec<u8>>,
    archive_name: String,
}

impl CoffArchiveBuilder {
    fn new(name: &str) -> Self {
        Self {
            symbols: IndexMap::new(),
            sections: vec![],
            archive_name: name.to_owned(),
        }
    }

    fn add_import_descriptors(&mut self) -> io::Result<()> {
        let (name, data) = build_import_descriptor(&self.archive_name)?;

        self.sections.push(data);
        self.symbols.insert(name, self.sections.len());

        let (name, data) = build_null_import_descriptor()?;

        self.sections.push(data);
        self.symbols.insert(name, self.sections.len());

        let (name, data) = build_null_thunk_data(&self.archive_name)?;

        self.sections.push(data);
        self.symbols.insert(name, self.sections.len());

        Ok(())
    }

    fn add_short_import(&mut self, rust_name: &str, import: &Import) -> io::Result<()> {
        let mut short_import = vec![];
        short_import.write_u16::<NativeEndian>(0x0000)?; // IMAGE_FILE_MACHINE_UNKNOWN
        short_import.write_u16::<NativeEndian>(0xFFFF)?; // Reserved
        short_import.write_u16::<NativeEndian>(0x0)?;    // Version
        let arch:u16 = arch();
        short_import.write_u16::<NativeEndian>(arch)?;   // Arch
        short_import.write_u32::<NativeEndian>(0x0)?;    // Time/Date (todo: actaul value)

        let item_name = import.name().unwrap_or_default();
        let dll_name = self.archive_name.as_str();

        let size = dll_name.len() + item_name.len() + 2;
        short_import.write_u32::<NativeEndian>(size as u32)?;
        let ordinal = import.ordinal().unwrap_or_default();
        short_import.write_u16::<NativeEndian>(ordinal)?;

        let import_type = 0x00; // IMPORT_CODE
        let import_name_type: u16 = if import.ordinal().is_some() {
            0x0 // IMPORT_ORDINAL
        } else {
            0x1 // IMPORT_NAME
        };
        short_import.write_u16::<NativeEndian>(import_type + (import_name_type << 2))?;
        short_import.write_all(item_name.as_bytes())?;
        short_import.write_u8(b'\0')?;
        short_import.write_all(dll_name.as_bytes())?;
        short_import.write_u8(b'\0')?;


        self.sections.push(short_import);
        self.symbols.insert(format!("__imp_{}", rust_name), self.sections.len());
        self.symbols.insert(rust_name.to_string(), self.sections.len());

        Ok(())
    }
}

fn build_import_descriptor(archive_name: &str) -> io::Result<(String, Vec<u8>)> {
    let name = if archive_name.ends_with(".dll") {
        &archive_name[.. archive_name.len() - 4]
    } else {
        &archive_name
    };

    let import_desc_name = format!("__IMPORT_DESCRIPTOR_{}", name);
    let null_import_data = "__NULL_IMPORT_DESCRIPTOR".to_owned();
    let null_thunk_data = format!("\u{7F}{}_NULL_THUNK_DATA", name);

    // import descriptor
    const N_SECTIONS: u16 = 2;
    const N_SYMBOLS: u32 = 7;
    const N_RECLOCATIONS: u16 = 3;

    let mut buffer = vec![];
    buffer.write_u16::<NativeEndian>(arch())?;
    buffer.write_u16::<NativeEndian>(N_SECTIONS)?;
    buffer.write_u32::<NativeEndian>(0)?; // TIMESTAMP

    let data_len = COFF_HEADER_LEN + N_SECTIONS as usize * COFF_SECTION_HEADER_LEN +
        // .idata$2
        20 + N_RECLOCATIONS as usize * 10 +
        // .idata$6
        archive_name.len() + 1;
    
    buffer.write_u32::<NativeEndian>(data_len as u32)?; // aka: symbol table start
    buffer.write_u32::<NativeEndian>(N_SYMBOLS)?;
    buffer.write_u16::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u16::<NativeEndian>(0)?; // charactaristics (TODO: fix for 32 bit)

    // first section header
    buffer.write_all(b".idata$2")?;
    buffer.write_u32::<NativeEndian>(0)?; // VirtualSize: always 0 for libs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u32::<NativeEndian>(0x14)?; // section size
    buffer.write_u32::<NativeEndian>((COFF_HEADER_LEN + 
        N_SECTIONS as usize * COFF_SECTION_HEADER_LEN) as u32)?; // start of section
    buffer.write_u32::<NativeEndian>((COFF_HEADER_LEN + 
            N_SECTIONS as usize * COFF_SECTION_HEADER_LEN + 0x14) as u32)?; // start of relocs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 
    buffer.write_u16::<NativeEndian>(N_RECLOCATIONS)?;
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u32::<NativeEndian>(0xC0300040)?; // TODO: label bitflags

    // second section header
    buffer.write_all(b".idata$6")?;
    buffer.write_u32::<NativeEndian>(0)?; // VirtualSize: always 0 for libs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u32::<NativeEndian>((archive_name.len() + 1) as u32)?; // section size
    buffer.write_u32::<NativeEndian>((data_len - (archive_name.len() + 1)) as u32)?; // start of section
    buffer.write_u32::<NativeEndian>(0)?; // start of relocs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u32::<NativeEndian>(0xC0200040)?; // TODO: label bitflags

    // .idata$2
    buffer.write_all(&[0; 0x14])?;

    //relocs
    //name rva
    buffer.write_u32::<NativeEndian>(0x0C)?;
    buffer.write_u32::<NativeEndian>(2)?;
    buffer.write_u16::<NativeEndian>(0x03)?; // IMAGE_REL_AMD64_ADDR32NB 
    //import lookup table rva
    buffer.write_u32::<NativeEndian>(0x00)?;
    buffer.write_u32::<NativeEndian>(3)?;
    buffer.write_u16::<NativeEndian>(0x03)?; // IMAGE_REL_AMD64_ADDR32NB 
    //import addr table rva
    buffer.write_u32::<NativeEndian>(0x10)?;
    buffer.write_u32::<NativeEndian>(4)?;
    buffer.write_u16::<NativeEndian>(0x03)?; // IMAGE_REL_AMD64_ADDR32NB 

    // .idata$6
    buffer.write_all(archive_name.as_bytes())?;
    buffer.write_u8(b'\0')?;

    let mut string_start = 4;
    let mut string_table = vec![];

    // symbol table
    write_symbol(&mut buffer, SymbolName::Offset(string_start),1, IMAGE_SYM_CLASS_EXTERNAL)?;
    write_symbol(&mut buffer, SymbolName::Name(".idata$2"), 1, IMAGE_SYM_CLASS_SECTION)?;
    write_symbol(&mut buffer, SymbolName::Name(".idata$6"),2, IMAGE_SYM_CLASS_STATIC)?;
    write_symbol(&mut buffer, SymbolName::Name(".idata$4"),0, IMAGE_SYM_CLASS_SECTION)?;
    write_symbol(&mut buffer, SymbolName::Name(".idata$5"),0, IMAGE_SYM_CLASS_SECTION)?;
    string_start += import_desc_name.len();
    string_table.write_all(import_desc_name.as_bytes())?;
    string_table.write_u8(b'\0')?;
    write_symbol(&mut buffer, SymbolName::Offset(string_start),0, IMAGE_SYM_CLASS_EXTERNAL)?;
    string_start += null_import_data.len();
    string_table.write_all( null_import_data.as_bytes())?;
    string_table.write_u8(b'\0')?;
    write_symbol(&mut buffer, SymbolName::Offset(string_start),0, IMAGE_SYM_CLASS_EXTERNAL)?;
    string_table.write_all( null_thunk_data.as_bytes())?;
    string_table.write_u8(b'\0')?;

    buffer.write_u32::<NativeEndian>(string_table.len() as u32)?;
    buffer.write_all(&string_table)?;
    if buffer.len() % 2 != 0 {
        buffer.write_u8(b'\0')?;
    }

    Ok((import_desc_name, buffer))
}

fn build_null_import_descriptor() -> io::Result<(String, Vec<u8>)> {
    let null_import_data = "__NULL_IMPORT_DESCRIPTOR".to_owned();

    // import descriptor
    const N_SECTIONS: u16 = 1;
    const N_SYMBOLS: u32 = 1;

    let mut buffer = vec![];
    buffer.write_u16::<NativeEndian>(arch())?;
    buffer.write_u16::<NativeEndian>(N_SECTIONS)?;
    buffer.write_u32::<NativeEndian>(0)?; // TIMESTAMP

    let data_len = COFF_HEADER_LEN + N_SECTIONS as usize * COFF_SECTION_HEADER_LEN +
        // .idata$3
        20;
    
    buffer.write_u32::<NativeEndian>(data_len as u32)?; // aka: symbol table start
    buffer.write_u32::<NativeEndian>(N_SYMBOLS)?;
    buffer.write_u16::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u16::<NativeEndian>(0)?; // charactaristics (TODO: fix for 32 bit)

    // first section header
    buffer.write_all(b".idata$3")?;
    buffer.write_u32::<NativeEndian>(0)?; // VirtualSize: always 0 for libs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u32::<NativeEndian>(0x14)?; // section size
    buffer.write_u32::<NativeEndian>((COFF_HEADER_LEN + 
        N_SECTIONS as usize * COFF_SECTION_HEADER_LEN) as u32)?; // start of section
    buffer.write_u32::<NativeEndian>(0)?; // start of relocs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u32::<NativeEndian>(0xC0300040)?; // TODO: label bitflags

    // .idata$3
    buffer.write_all(&[0; 0x14])?;

    let string_start = 4;
    let mut string_table = vec![];

    // symbol table
    write_symbol(&mut buffer, SymbolName::Offset(string_start),1, IMAGE_SYM_CLASS_EXTERNAL)?;
    string_table.write_all( null_import_data.as_bytes())?;
    string_table.write_u8(b'\0')?;

    buffer.write_u32::<NativeEndian>(string_table.len() as u32)?;
    buffer.write_all(&string_table)?;
    if buffer.len() % 2 != 0 {
        buffer.write_u8(b'\0')?;
    }

    Ok((null_import_data, buffer))
}

fn build_null_thunk_data(archive_name: &str) -> io::Result<(String, Vec<u8>)> {
    let name = if archive_name.ends_with(".dll") {
        &archive_name[.. archive_name.len() - 4]
    } else {
        &archive_name
    };

    let null_thunk_data = format!("\u{7F}{}_NULL_THUNK_DATA", name);

    //todo: 32bit
    let va_size = 8;

    // import descriptor
    const N_SECTIONS: u16 = 2;
    const N_SYMBOLS: u32 = 1;

    let mut buffer = vec![];
    buffer.write_u16::<NativeEndian>(arch())?;
    buffer.write_u16::<NativeEndian>(N_SECTIONS)?;
    buffer.write_u32::<NativeEndian>(0)?; // TIMESTAMP

    let data_len = COFF_HEADER_LEN + N_SECTIONS as usize * COFF_SECTION_HEADER_LEN +
        // .idata$5
        va_size +
        // .idata$4
        va_size;
    
    buffer.write_u32::<NativeEndian>(data_len as u32)?; // aka: symbol table start
    buffer.write_u32::<NativeEndian>(N_SYMBOLS)?;
    buffer.write_u16::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u16::<NativeEndian>(0)?; // charactaristics (TODO: fix for 32 bit)

    // first section header
    buffer.write_all(b".idata$5")?;
    buffer.write_u32::<NativeEndian>(0)?; // VirtualSize: always 0 for libs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u32::<NativeEndian>(va_size as u32)?; // section size
    buffer.write_u32::<NativeEndian>((COFF_HEADER_LEN + 
        N_SECTIONS as usize * COFF_SECTION_HEADER_LEN) as u32)?; // start of section
    buffer.write_u32::<NativeEndian>(0)?; // start of relocs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u32::<NativeEndian>(0xC0400040)?; // TODO: label bitflags

    // second section header
    buffer.write_all(b".idata$4")?;
    buffer.write_u32::<NativeEndian>(0)?; // VirtualSize: always 0 for libs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 for libs
    buffer.write_u32::<NativeEndian>(va_size as u32)?; // section size
    buffer.write_u32::<NativeEndian>((COFF_HEADER_LEN + 
        N_SECTIONS as usize * COFF_SECTION_HEADER_LEN) as u32 + va_size as u32)?; // start of section
    buffer.write_u32::<NativeEndian>(0)?; // start of relocs
    buffer.write_u32::<NativeEndian>(0)?; // always 0 
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u16::<NativeEndian>(0)?;
    buffer.write_u32::<NativeEndian>(0xC0400040)?; // TODO: label bitflags

    // .idata$5, ILT
    buffer.write_u64::<NativeEndian>(0)?;

    // .idata$4, IAT
    buffer.write_u64::<NativeEndian>(0)?;

    //symbols
    let string_start = 4;
    let mut string_table = vec![];

    write_symbol(&mut buffer, SymbolName::Offset(string_start),1, IMAGE_SYM_CLASS_EXTERNAL)?;
    string_table.write_all( null_thunk_data.as_bytes())?;
    string_table.write_u8(b'\0')?;

    buffer.write_u32::<NativeEndian>(string_table.len() as u32)?;
    buffer.write_all(&string_table)?;
    if buffer.len() % 2 != 0 {
        buffer.write_u8(b'\0')?;
    }

    Ok((null_thunk_data, buffer))
}


enum SymbolName<'a> {
    Name(&'a str),
    Offset(usize),
}

fn write_symbol<W: Write>(buf: &mut W, name: SymbolName, section: u16, sym_ty: u8) -> io::Result<()> {
    match name {
        SymbolName::Name(name) => {
            buf.write_all(&name.as_bytes()[0..8])?;
        },
        SymbolName::Offset(o) => {
            buf.write_u32::<NativeEndian>(0x00)?;
            buf.write_u32::<NativeEndian>(o as u32)?;
        },
    }

    buf.write_u32::<NativeEndian>(0x00)?;
    buf.write_u16::<NativeEndian>(section)?;
    buf.write_u16::<NativeEndian>(0x00)?;
    buf.write_u8(sym_ty)?;
    buf.write_u8(0x00)?;

    Ok(())
}