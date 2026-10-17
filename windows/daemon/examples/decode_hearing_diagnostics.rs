#[path = "../src/hearing_diagnostics.rs"]
mod hearing_diagnostics;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("Usage: decode_hearing_diagnostics <71-byte-record> [product-id-hex]")?;
    let product = args
        .next()
        .map(|id| u32::from_str_radix(id.trim_start_matches("0x"), 16))
        .transpose()?;
    if args.next().is_some() {
        return Err("Too many arguments".into());
    }
    let record = hearing_diagnostics::parse(&std::fs::read(path)?, product)?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}
