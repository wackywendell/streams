//! Black-box local capture probe; no native attestation, inferred enrollment or remote reads.

use std::{io, path::Path};

use serde::Deserialize;
use serde_json::json;
use streams_temper::{
    agent::{ContextRequest, context},
    codec::load_json,
    config::{HostIdentity, source_id},
    runtime::conversations::Harness,
    sync::local::LocalCatalog,
    yaml_store::YamlStore,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    harness: Harness,
    native_id: String,
    stream_id: Option<String>,
    environment_stream_id: Option<String>,
    workspace_stream_id: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: agent_context ROOT HOST_UUID".into());
    }
    let request: Request = serde_json::from_value(load_json(&io::read_to_string(io::stdin())?)?)?;
    let host = HostIdentity::parse(&args[1])?;
    let root = Path::new(&args[0]);
    let backend = LocalCatalog::new(YamlStore::new(root), host.as_str())?;
    let source = source_id(root, &host)?;
    let result = context(
        &backend,
        host.as_str(),
        &source,
        ContextRequest {
            harness: request.harness,
            native_id: &request.native_id,
            stream_id: request.stream_id.as_deref(),
            environment_stream_id: request.environment_stream_id.as_deref(),
            workspace_stream_id: request.workspace_stream_id.as_deref(),
        },
    );
    let response = match result {
        Ok(value) => json!({"context": value}),
        Err(error) => json!({"error": error.to_string()}),
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}
