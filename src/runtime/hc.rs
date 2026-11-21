//! Save notice on the caller's hc terminal, using the fixed `streams.saved` label.

use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use crate::{
    frontend::{
        FrontendError,
        hc::{HcClient, HcEndpoint},
    },
    runtime::Result,
};

/// Fixed hc terminal label written with a fresh token after any catalog save.
pub const SAVE_NOTICE_LABEL: &str = "streams.saved";

/// Emit a save notice on the caller's hc terminal (`HC_TERMINAL_ID`) so reactive views refresh.
/// Outside an hc terminal, does nothing and returns `Ok(())`.
pub fn notify_saved(
    environment: &BTreeMap<String, String>,
    cwd: &Path,
    timeout_seconds: f64,
) -> Result<()> {
    let Some(terminal) = environment
        .get("HC_TERMINAL_ID")
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if terminal.contains('/') || crate::model::check_text(terminal, "terminal", false).is_err() {
        return Err(FrontendError::invalid("invalid hc terminal identifier").into());
    }
    let endpoint = HcEndpoint::resolve(environment, cwd)?;
    let mut client = HcClient::connect(endpoint, None, timeout_seconds, None)?;
    let token = uuid::Uuid::new_v4().to_string();
    let params = json!({
        "terminal": terminal,
        "labels": {
            SAVE_NOTICE_LABEL: token,
        },
    });
    let (_, result) = client.exchange("terminal.label", &params, true);
    let reply = result.map_err(|failure| failure.error)?;
    let confirmed = reply.get("address").and_then(Value::as_str) == Some(terminal.as_str())
        && reply.get("state").and_then(Value::as_str) == Some("live")
        && reply
            .get("labels")
            .and_then(Value::as_object)
            .and_then(|labels| labels.get(SAVE_NOTICE_LABEL))
            .and_then(Value::as_str)
            == Some(token.as_str());
    if !confirmed {
        return Err(FrontendError::invalid("hc save notice was not confirmed").into());
    }
    Ok(())
}
