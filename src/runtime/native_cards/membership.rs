use super::*;
use crate::runtime::{
    native::PiCallerObservation,
    placement::HerdrPlacementObservation,
    state::{NativePlacement, RuntimeState},
};
use crate::{agent::RecordContext, model::Stream, operations::membership::MoveIntent};

/// Native membership inputs bound to one caller proof and one addressed placement.
/// Actual destination ownership and membership checks remain inside the catalog save.
pub struct NativeMembershipObservation<'a> {
    catalog_source: &'a str,
    host_id: &'a str,
    caller: &'a PiCallerObservation,
    placement: Option<NativePlacement>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeMembershipPlan {
    /// No destination selected; this does not admit binding or resource creation.
    Unassociated,
    /// Agreeing missing paint is a desired difference, not permission to move yet.
    NeedsProjection {
        stream_id: String,
        workspace_id: String,
        missing: Tokens,
    },
    Ready(MoveIntent),
}

impl<'a> NativeMembershipObservation<'a> {
    /// Metadata selects one canonical Stream row. Without that selector, only an
    /// actual current Session/Stream may name a placement, never an ambient hint.
    /// The driver holds its native writer guard before collecting the caller snapshot.
    pub fn observe(
        records: &'a RecordContext,
        runtime: &'a RuntimeState,
        caller: &'a PiCallerObservation,
    ) -> Result<Self> {
        super::super::text(&records.catalog_source)?;
        let current = current_stream(records, runtime.host_id(), caller)?;
        let workspace = caller.calling_workspace();
        let tokens = caller
            .snapshot()
            .workspace_tokens(workspace.workspace_id())
            .expect("observed caller workspace");
        let selected = tokens
            .get("strm_stream")
            .map(String::as_str)
            .or(current.map(|stream| stream.id.as_str()));
        let placement = selected
            .map(|stream| runtime.placement(workspace.scope(), stream))
            .transpose()?
            .flatten();
        Ok(Self {
            catalog_source: &records.catalog_source,
            host_id: runtime.host_id(),
            caller,
            placement,
        })
    }

    pub fn caller(&self) -> &PiCallerObservation {
        self.caller
    }

    pub(super) fn admit_context(&self, records: &RecordContext) -> Result<Scope> {
        if records.catalog_source != self.catalog_source {
            return Err(invalid("native pass catalog source changed"));
        }
        let stream = current_stream(records, self.host_id, self.caller)?
            .ok_or_else(|| invalid("native pass requires an enrolled Session"))?;
        let session = records.session.as_ref().expect("checked current Session");
        let NativeMembershipPlan::Ready(intent) = self.plan()? else {
            return Err(invalid(
                "native pass requires a complete destination placement",
            ));
        };
        if intent.destination_stream_id() != session.stream_id {
            return Err(invalid(
                "native destination differs from current Session membership",
            ));
        }
        Ok(Scope {
            catalog_source: records.catalog_source.clone(),
            host_id: records.host_id.clone(),
            stream_id: stream.id.clone(),
            session_id: session.id.clone(),
            harness: session.harness.clone(),
            native_session_id: session.harness_session_id.clone(),
        })
    }

    /// No record, transcript or native reads. A ready intent is still subject to
    /// apply_move's actual Session/source/destination identity and host checks.
    pub fn plan(&self) -> Result<NativeMembershipPlan> {
        let workspace = self.caller.calling_workspace();
        let workspace_id = workspace.workspace_id();
        let tokens = self
            .caller
            .snapshot()
            .workspace_tokens(workspace_id)
            .expect("observed caller workspace");
        let painted = ["strm_catalog", "strm_host", "strm_stream", "strm_binding"]
            .iter()
            .any(|key| tokens.contains_key(*key));
        let Some(placement) = &self.placement else {
            if painted {
                return Err(invalid("unmatched native workspace binding retained"));
            }
            return Ok(NativeMembershipPlan::Unassociated);
        };
        if placement.pending().is_some() {
            return Err(invalid("pending native workspace attempt retained"));
        }
        if placement
            .binding()
            .is_none_or(|binding| binding.workspace_id() != workspace_id)
        {
            // A conversation may have moved away from its current Stream's
            // workspace. That other workspace's metadata is not an input here.
            if !painted {
                return Ok(NativeMembershipPlan::Unassociated);
            }
            return Err(invalid("native placement names another workspace"));
        }
        let observed = HerdrPlacementObservation::new(
            self.catalog_source,
            self.host_id,
            workspace.scope(),
            placement,
            self.caller.snapshot(),
        )?;
        if !observed.missing_tokens().is_empty() {
            return Ok(NativeMembershipPlan::NeedsProjection {
                stream_id: placement.stream_id().to_owned(),
                workspace_id: workspace_id.to_owned(),
                missing: observed.missing_tokens().clone(),
            });
        }
        observed.require_ready()?;
        MoveIntent::new(
            self.host_id,
            "pi",
            self.caller.caller().native_session_id(),
            placement.stream_id(),
        )
        .map(NativeMembershipPlan::Ready)
        .map_err(|error| invalid(error.to_string()))
    }
}

fn current_stream<'a>(
    records: &'a RecordContext,
    host: &str,
    caller: &PiCallerObservation,
) -> Result<Option<&'a Stream>> {
    if records.host_id != host {
        return Err(invalid("native membership requires the fixed local host"));
    }
    let Some(session) = &records.session else {
        return Ok(None);
    };
    let stream = records
        .stream
        .as_ref()
        .ok_or_else(|| invalid("enrolled native Session has no current Stream"))?;
    session
        .validate_portable()
        .map_err(|error| invalid(error.to_string()))?;
    stream
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    if session.host_id != host
        || stream.owner_host_id != host
        || session.stream_id != stream.id
        || session.harness != "pi"
        || session.harness_session_id != caller.caller().native_session_id()
    {
        return Err(invalid(
            "native pass requires the exact local conversation and current Stream",
        ));
    }
    Ok(Some(stream))
}

#[cfg(test)]
mod tests;
