use psd_great::{read_psd, write_psd, PsdError, ReadOptions, Result, WriteOptions};
use std::env;
use std::fs;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: cargo run --example resave_psd -- <input.psd> <output.psd>");
        std::process::exit(2);
    }

    let input = &args[1];
    let output = &args[2];

    let file = fs::File::open(input).map_err(PsdError::Io)?;
    let psd = read_psd(file, ReadOptions::default())?;
    let bytes = write_psd(&psd, &WriteOptions::default())?;
    fs::write(output, bytes).map_err(PsdError::Io)?;

    println!("{output}");
    Ok(())
}
