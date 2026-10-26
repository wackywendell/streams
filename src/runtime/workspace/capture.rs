use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Session, Stream};
use crate::runtime::{
    Result,
    arrangements::{self, Arrangement, VerifiedAgent},
    invalid,
    native::PiCallerObservation,
    placement::ReadyHerdrPlacement,
};

/// One admitted workspace snapshot and its verified conversation inputs.
/// Its keys select reads, never membership or native origin authority.
pub struct WorkspaceCaptureObservation<'a> {
    stream: &'a Stream,
    placement: ReadyHerdrPlacement<'a>,
    agents: Vec<VerifiedAgent<'a>>,
    session_keys: BTreeSet<String>,
}

impl<'a> WorkspaceCaptureObservation<'a> {
    /// Explicit inventory reads identity files only in the concerned workspace.
    pub fn observe(stream: &'a Stream, placement: ReadyHerdrPlacement<'a>) -> Result<Self> {
        Self::check_stream(stream, &placement)?;
        let agents = arrangements::verified_inventory(
            placement.snapshot().topology(),
            Some(placement.workspace_id()),
        )?;
        Ok(Self::from_inventory(stream, placement, agents))
    }

    /// Reuse the already-verified owner's identity from the SAME snapshot proof.
    pub fn from_caller(
        stream: &'a Stream,
        placement: ReadyHerdrPlacement<'a>,
        caller: &'a PiCallerObservation,
    ) -> Result<Self> {
        Self::check_stream(stream, &placement)?;
        if caller.calling_workspace().scope() != placement.placement().scope() {
            return Err(invalid(
                "workspace capture endpoint differs from caller proof",
            ));
        }
        if !std::ptr::eq(placement.snapshot(), caller.snapshot()) {
            return Err(invalid(
                "workspace capture requires the same caller snapshot",
            ));
        }
        let agents =
            arrangements::verified_inventory_from_caller(caller, placement.workspace_id())?;
        Ok(Self::from_inventory(stream, placement, agents))
    }

    fn check_stream(stream: &Stream, placement: &ReadyHerdrPlacement<'_>) -> Result<()> {
        stream
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        if stream.owner_host_id != placement.host_id()
            || stream.id != placement.placement().stream_id()
        {
            return Err(invalid(
                "workspace capture requires the actual fixed-local Stream",
            ));
        }
        Ok(())
    }

    fn from_inventory(
        stream: &'a Stream,
        placement: ReadyHerdrPlacement<'a>,
        agents: Vec<VerifiedAgent<'a>>,
    ) -> Self {
        let session_keys = agents
            .iter()
            .map(|agent| {
                Session::id_for(
                    placement.host_id(),
                    agent.harness.as_ref(),
                    &agent.identity.id,
                )
            })
            .collect();
        Self {
            stream,
            placement,
            agents,
            session_keys,
        }
    }

    pub fn session_keys(&self) -> &BTreeSet<String> {
        &self.session_keys
    }

    /// DRIVER supplies every exact read, including explicit None for unenrolled agents.
    /// Assembly is pure: no catalog/native/identity/configuration reads or callbacks.
    pub fn plan(&self, sessions: &BTreeMap<String, Option<Session>>) -> Result<Arrangement> {
        if sessions.len() != self.session_keys.len()
            || !self
                .session_keys
                .iter()
                .all(|key| sessions.contains_key(key))
        {
            return Err(invalid(
                "workspace capture requires exactly its named Session reads",
            ));
        }
        for agent in &self.agents {
            let key = Session::id_for(
                self.placement.host_id(),
                agent.harness.as_ref(),
                &agent.identity.id,
            );
            if let Some(session) = &sessions[&key] {
                session
                    .validate_portable()
                    .map_err(|error| invalid(error.to_string()))?;
                if session.id != key
                    || session.host_id != self.placement.host_id()
                    || session.harness != agent.harness.as_ref()
                    || session.harness_session_id != agent.identity.id
                {
                    return Err(invalid(
                        "workspace capture Session differs from its named native identity",
                    ));
                }
            }
        }
        arrangements::capture_observed(
            self.placement.snapshot().topology(),
            self.placement.workspace_id(),
            self.stream,
            sessions.values().filter_map(Option::as_ref),
            self.placement.host_id(),
            &self.agents,
        )
    }
}

#[cfg(test)]
mod tests;
