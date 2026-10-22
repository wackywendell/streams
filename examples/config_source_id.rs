//! Black-box probe for Python's physical catalog-source fingerprint contract.

use std::path::Path;
use streams_temper::config::{HostIdentity, source_id};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: config_source_id ROOT HOST_UUID".into());
    }
    println!(
        "{}",
        source_id(Path::new(&args[0]), &HostIdentity::parse(&args[1])?)?
    );
    Ok(())
}
