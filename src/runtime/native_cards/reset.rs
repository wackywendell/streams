//! Explicit offline cutover, never called by an ordinary read or native action.

use super::super::native_state::LegacyEvidence;
use super::*;

#[derive(Debug, Serialize)]
pub struct EvidenceResetReport {
    pub already_completed: bool,
    pub closed_cards: Vec<String>,
    pub cleared_pair_markers: Vec<String>,
    pub deleted_evidence: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum ResetAction {
    CloseCard(NativeTuple),
    ClearPair(NativeTuple),
}

/// Classification never comes from the opaque old key. Unmarked cards self-exit;
/// treating their indistinguishable editor records as cards would risk unsaved work.
fn plan(snapshot: &HerdrNativeSnapshot, records: &[LegacyEvidence]) -> Vec<ResetAction> {
    let mut close = BTreeMap::new();
    for record in records {
        let Some((nonce, resources)) = &record.observed else {
            continue;
        };
        let Some(pane) = snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| NativeTuple::from_pane(pane).matches_resources(resources))
        else {
            continue;
        };
        let tokens = snapshot
            .pane_tokens(&pane.pane.id)
            .expect("snapshot pane tokens");
        if tokens.get("strm_role").map(String::as_str) == Some("card")
            && tokens.get("strm_nonce") == Some(nonce)
            && tokens.get("strm_card") == Some(&pane.terminal_id)
            && PAIR_KEYS
                .iter()
                .all(|key| tokens.get(*key).is_some_and(|v| !v.is_empty()))
            && !tokens.contains_key("strm_editor")
            && !tokens.contains_key("strm_doc")
        {
            close.insert(pane.pane.id.clone(), NativeTuple::from_pane(pane));
        }
    }
    let mut actions: Vec<_> = close
        .values()
        .cloned()
        .map(ResetAction::CloseCard)
        .collect();
    for pane in &snapshot.topology().panes {
        if !close.contains_key(&pane.pane.id)
            && snapshot
                .pane_tokens(&pane.pane.id)
                .is_some_and(|tokens| PAIR_KEYS.iter().any(|key| tokens.contains_key(*key)))
        {
            actions.push(ResetAction::ClearPair(NativeTuple::from_pane(pane)));
        }
    }
    actions
}

/// Operator preconditions: backup, stopped Pi children/serve and pre-rule cards.
/// No source/catalog reads, provenance guesses, confirmation snapshots or retries.
pub fn reset_evidence(
    client: &HerdrClient,
    state: &NativeState,
    legacy: &NativeState,
    timeout: f64,
    cancelled: &AtomicBool,
) -> Result<EvidenceResetReport> {
    let lock = state.lock(timeout, cancelled)?;
    let mut report = EvidenceResetReport {
        already_completed: state.format_completed()?,
        closed_cards: Vec::new(),
        cleared_pair_markers: Vec::new(),
        deleted_evidence: 0,
    };
    if report.already_completed {
        return Ok(report);
    }
    let records = lock.legacy_evidence(legacy)?;
    let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot()?)?;
    for action in plan(&snapshot, &records) {
        if cancelled.load(Ordering::Acquire) {
            return Err(invalid("native evidence reset cancelled"));
        }
        match action {
            ResetAction::CloseCard(card) => {
                let (reply, _) = client.card_request(
                    "plugin.pane.close",
                    serde_json::json!({"pane_id": card.pane_id}),
                    Some("plugin_pane_closed"),
                    true,
                )?;
                if reply["pane_id"] != card.pane_id {
                    return Err(invalid(
                        "reset card close reply mismatched; evidence retained",
                    ));
                }
                report.closed_cards.push(card.pane_id);
            }
            ResetAction::ClearPair(pane) => {
                let tokens: BTreeMap<_, _> =
                    PAIR_KEYS.iter().map(|key| (*key, Value::Null)).collect();
                client.card_request(
                    "pane.report_metadata",
                    serde_json::json!({"pane_id":pane.pane_id,"source":PLUGIN_ID,"tokens":tokens}),
                    Some("ok"),
                    true,
                )?;
                report.cleared_pair_markers.push(pane.pane_id);
            }
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(invalid("native evidence reset cancelled"));
    }
    lock.finish_reset(legacy, &records)?;
    report.deleted_evidence = records.len();
    Ok(report)
}

#[cfg(test)]
mod tests;
