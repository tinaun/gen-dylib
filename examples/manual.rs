use gen_dylib::{ImportLibBuilder};

use std::fs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>>{
    let lib_file = ImportLibBuilder::new("mydll.a.dll")
        .import_ordinal("mult", 3)
        .import_name("add", "add")
        .import_name("sub", "sub")
        .build();

    println!("{:x?}", lib_file);

    fs::write("mydll.test.lib", lib_file)?;

    Ok(())
}