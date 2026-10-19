//! JSON-lines adapter for the Python differential tests, not a user-facing CLI.

use std::io::{self, BufRead, Write};

use serde::Deserialize;
use serde_json::{Value, json};
use streams_temper::{
    codec::record_to_data,
    sync::{envelope::decode_operation, local::LocalCatalog},
    yaml_store::YamlStore,
};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum Request {
    Save { message: String },
    Snapshot,
}

fn execute(local: &LocalCatalog, line: &str) -> Result<Value, String> {
    let request: Request = serde_json::from_str(line).map_err(|error| error.to_string())?;
    let mut response = json!({});
    if let Request::Save { message } = request {
        let (operation, origin) = decode_operation(&message).map_err(|error| error.to_string())?;
        let result = local
            .save(&operation, Some(&origin))
            .map_err(|error| error.to_string())?;
        // Record's derived serde representation is not the persisted wire format.
        let domain = result.domain.as_ref().map(|domain| {
            json!({
                "operation_id": domain.operation_id,
                "outcome": domain.outcome,
                "record": domain.record.as_ref().map(record_to_data),
                "conflicts": domain.conflicts,
                "reason": domain.reason,
            })
        });
        response["result"] = json!({
            "sharing": result.sharing,
            "local": result.local,
            "domain": domain,
            "local_revision": result.local_revision,
            "remote_revision": result.remote_revision,
            "detail": result.detail,
            "conflicts": result.conflicts,
        });
    }
    let snapshot = local.snapshot().map_err(|error| error.to_string())?;
    let mut records = snapshot.catalog.records();
    records.sort_by_key(|record| record.key());
    response["records"] = Value::Array(
        records
            .iter()
            .map(|record| json!({"kind": record.kind_str(), "record": record_to_data(record)}))
            .collect(),
    );
    Ok(response)
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let local = match args.as_slice() {
        [root, host] => {
            LocalCatalog::new(YamlStore::new(root), host.clone()).map_err(|error| error.to_string())
        }
        _ => Err("expected catalog root and host arguments".to_string()),
    };
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let response = match line {
            Ok(line) => match &local {
                Ok(local) => execute(local, &line).unwrap_or_else(|error| json!({"error": error})),
                Err(error) => json!({"error": error}),
            },
            Err(error) => json!({"error": error.to_string()}),
        };
        if serde_json::to_writer(&mut stdout, &response).is_err()
            || writeln!(stdout).is_err()
            || stdout.flush().is_err()
        {
            break;
        }
    }
}
