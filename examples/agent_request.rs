//! Black-box captured-request adapter; stdin is the unchanged harness request.

use std::io::{self, Read};

use serde_json::json;
use streams_temper::{agent::parse_update, sync::envelope::encode_operation};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = std::env::args().nth(1).ok_or("expected catalog source")?;
    let mut request = String::new();
    io::stdin().read_to_string(&mut request)?;
    let response = match parse_update(&request, &source)
        .and_then(|operation| encode_operation(&operation, None))
    {
        Ok(message) => json!({"message": message}),
        Err(error) => json!({"error": error.to_string()}),
    };
    println!("{response}");
    Ok(())
}
