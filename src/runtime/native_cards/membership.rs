use super::*;
use crate::operations::membership::MoveIntent;
use crate::runtime::{
    native::PiCallerObservation,
    state::{Association, EnrollmentClaim, RuntimeState},
};

/// The native inputs to membership admission, bound to one caller observation.
/// Catalog identity and host checks remain inside the record-local save.
pub struct NativeMembershipObservation<'a> {
    catalog_source: &'a str,
    host_id: &'a str,
    caller: &'a PiCallerObservation,
    association: Option<Association>,
    claim: Option<EnrollmentClaim>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeMembershipPlan {
    /// No destination selected; this does not admit binding or resource creation.
    Unassociated,
    Ready(MoveIntent),
}

impl<'a> NativeMembershipObservation<'a> {
    /// Read only the two relationship records addressed by this exact caller.
    /// The driver holds its native writer guard before collecting the caller snapshot.
    pub fn observe(
        catalog_source: &'a str,
        runtime: &'a RuntimeState,
        caller: &'a PiCallerObservation,
    ) -> Result<Self> {
        super::super::text(catalog_source)?;
        let workspace = caller.calling_workspace();
        Ok(Self {
            catalog_source,
            host_id: runtime.host_id(),
            caller,
            association: runtime.association(workspace.scope(), workspace.workspace_id())?,
            claim: runtime.enrollment_claim(workspace.scope(), workspace.workspace_id())?,
        })
    }

    pub fn caller(&self) -> &PiCallerObservation {
        self.caller
    }

    pub(super) fn admit_context(&self, records: &crate::agent::RecordContext) -> Result<Scope> {
        let session = records
            .session
            .as_ref()
            .ok_or_else(|| invalid("native pass requires an enrolled Session"))?;
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
        if records.catalog_source != self.catalog_source
            || records.host_id != self.host_id
            || session.host_id != self.host_id
            || stream.owner_host_id != self.host_id
            || session.stream_id != stream.id
            || session.harness != "pi"
            || session.harness_session_id != self.caller.caller().native_session_id()
        {
            return Err(invalid(
                "native pass requires the exact local conversation and current Stream",
            ));
        }
        let NativeMembershipPlan::Ready(intent) = self.plan()? else {
            return Err(invalid(
                "native pass requires a ready destination association",
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
        let workspace_id = self.caller.caller().workspace_id();
        let tokens = self
            .caller
            .snapshot()
            .workspace_tokens(workspace_id)
            .expect("observed caller workspace");
        let painted = tokens.keys().any(|key| key.starts_with("strm_"));
        let Some(association) = &self.association else {
            if self.claim.is_some() || painted {
                return Err(invalid(
                    "unmatched native workspace claim or binding retained",
                ));
            }
            return Ok(NativeMembershipPlan::Unassociated);
        };
        if !association.ready() || association.workspace_id() != workspace_id {
            return Err(invalid("membership requires a ready destination workspace"));
        }
        if self.claim.as_ref().is_some_and(|claim| {
            claim.workspace_id() != workspace_id || claim.stream_id() != association.stream_id()
        }) {
            return Err(invalid(
                "destination association and enrollment claim disagree",
            ));
        }
        let expected: Tokens = BTreeMap::from([
            ("strm_catalog".into(), json_digest(&self.catalog_source)?),
            ("strm_host".into(), self.host_id.to_owned()),
            ("strm_stream".into(), association.stream_id().to_owned()),
        ]);
        if painted
            && (tokens.get("strm_binding").is_none_or(String::is_empty)
                || expected
                    .iter()
                    .any(|(key, value)| tokens.get(key) != Some(value)))
        {
            return Err(invalid("conflicting native destination workspace binding"));
        }
        MoveIntent::new(
            self.host_id,
            "pi",
            self.caller.caller().native_session_id(),
            association.stream_id(),
        )
        .map(NativeMembershipPlan::Ready)
        .map_err(|error| invalid(error.to_string()))
    }
}

#[cfg(test)]
mod tests;
