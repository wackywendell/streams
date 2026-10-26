//! Pure native placement observations. Neither metadata nor a placement proves Pi origin.

use std::collections::BTreeMap;

use crate::frontend::herdr::HerdrNativeSnapshot;

use super::{Result, digest, invalid, state::NativePlacement, text};

type Tokens = BTreeMap<String, String>;

/// Desired binding versus this exact snapshot; missing paint is not readiness.
/// No record, transcript, frontend or configuration I/O occurs here.
/// Destination catalog ownership/membership stays inside the actual catalog save.
pub struct HerdrPlacementObservation<'a> {
    host_id: &'a str,
    placement: &'a NativePlacement,
    snapshot: &'a HerdrNativeSnapshot,
    missing: Tokens,
}

impl<'a> HerdrPlacementObservation<'a> {
    pub fn new(
        catalog_source: &str,
        host_id: &'a str,
        scope: &str,
        placement: &'a NativePlacement,
        snapshot: &'a HerdrNativeSnapshot,
    ) -> Result<Self> {
        text(catalog_source)?;
        text(host_id)?;
        if placement.scope() != scope {
            return Err(invalid(
                "native placement endpoint differs from observation",
            ));
        }
        if placement.pending().is_some() {
            return Err(invalid("pending native workspace attempt retained"));
        }
        let binding = placement
            .binding()
            .ok_or_else(|| invalid("native placement has no workspace binding"))?;
        let nonce = binding
            .nonce()
            .ok_or_else(|| invalid("CMUX placement has no Herdr binding capability"))?;
        let observed = snapshot
            .workspace_tokens(binding.workspace_id())
            .ok_or_else(|| invalid("native placement workspace absent from snapshot"))?;
        let desired = BTreeMap::from([
            (
                "strm_catalog".to_owned(),
                digest(
                    &serde_json::to_vec(catalog_source)
                        .map_err(|error| invalid(error.to_string()))?,
                ),
            ),
            ("strm_host".to_owned(), host_id.to_owned()),
            ("strm_stream".to_owned(), placement.stream_id().to_owned()),
            ("strm_binding".to_owned(), nonce.to_owned()),
        ]);
        let mut missing = BTreeMap::new();
        for (key, value) in desired {
            match observed.get(&key) {
                Some(actual) if *actual != value => {
                    return Err(invalid(
                        "conflicting native workspace binding; placement retained",
                    ));
                }
                Some(_) => {}
                None => {
                    missing.insert(key, value);
                }
            }
        }
        Ok(Self {
            host_id,
            placement,
            snapshot,
            missing,
        })
    }

    pub fn workspace_id(&self) -> &str {
        self.placement
            .binding()
            .expect("admitted workspace binding")
            .workspace_id()
    }

    pub fn missing_tokens(&self) -> &Tokens {
        &self.missing
    }

    pub fn require_ready(self) -> Result<ReadyHerdrPlacement<'a>> {
        if !self.missing.is_empty() {
            return Err(invalid("native workspace binding projection is incomplete"));
        }
        Ok(ReadyHerdrPlacement(self))
    }
}

/// Matching complete projection with no pending workspace attempt, not origin proof.
/// The snapshot and canonical placement cannot be replaced independently.
pub struct ReadyHerdrPlacement<'a>(HerdrPlacementObservation<'a>);

impl<'a> ReadyHerdrPlacement<'a> {
    pub fn host_id(&self) -> &'a str {
        self.0.host_id
    }
    pub fn placement(&self) -> &'a NativePlacement {
        self.0.placement
    }
    pub fn snapshot(&self) -> &'a HerdrNativeSnapshot {
        self.0.snapshot
    }
    pub fn workspace_id(&self) -> &str {
        self.0.workspace_id()
    }
}

#[cfg(test)]
mod tests;
