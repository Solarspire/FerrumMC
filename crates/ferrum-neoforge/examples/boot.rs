use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let launch = PathBuf::from(args.next().ok_or("missing launch JSON")?);
    let runtime = PathBuf::from(args.next().ok_or("missing runtime JSON")?);
    let output = PathBuf::from(args.next().ok_or("missing manifest output path")?);
    let runtime = ferrum_neoforge::Runtime::boot(&launch, &runtime)?;
    std::fs::write(output, runtime.registry_manifest()?)?;
    runtime.shutdown()?;
    Ok(())
}
