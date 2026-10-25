//! Complete projections for the embedded verification tables.

use super::state::RepairRequired;
use std::sync::OnceLock;
use temper_jit::table::{EvalContext, TransitionTable};

pub const CATALOG_PROTOCOL_IOA: &str = include_str!("../../specs/catalog_protocol.ioa.toml");

/// Parse the embedded specification once; a malformed shipped spec is a programming error.
pub fn protocol_table() -> &'static TransitionTable {
    static TABLE: OnceLock<TransitionTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        TransitionTable::try_from_ioa_source(CATALOG_PROTOCOL_IOA)
            .expect("embedded catalog protocol must parse")
    })
}

/// Reject missing projections instead of using Temper's false/zero defaults.
pub fn verify_total_context(
    table: &TransitionTable,
    ctx: &EvalContext,
) -> Result<(), RepairRequired> {
    for name in table.initial_values.booleans.keys() {
        if !ctx.booleans.contains_key(name) {
            return Err(RepairRequired::new(format!(
                "missing boolean projection: {name}"
            )));
        }
    }
    for name in table.initial_values.counters.keys() {
        if !ctx.counters.contains_key(name) {
            return Err(RepairRequired::new(format!(
                "missing counter projection: {name}"
            )));
        }
    }
    for name in table.initial_values.lists.keys() {
        if !ctx.lists.contains_key(name) {
            return Err(RepairRequired::new(format!(
                "missing list projection: {name}"
            )));
        }
    }
    for name in table.guard_field_names(None) {
        if !ctx.fields.contains_key(&name) {
            return Err(RepairRequired::new(format!(
                "missing field projection: {name}"
            )));
        }
    }
    Ok(())
}
