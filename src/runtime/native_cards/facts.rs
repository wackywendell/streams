use super::*;

/// Native facts shared by planners, independent of card rendering.
pub(super) struct NativeFacts<'a> {
    pub snapshot: &'a HerdrNativeSnapshot,
    pub caller: &'a NativeTuple,
    pub owner: &'a NativeTuple,
    pub key: &'a str,
    pub native_id: &'a str,
    pub binding: &'a Tokens,
}

impl<'a> NativeFacts<'a> {
    pub fn pair(&self) -> Result<PairState<'a>> {
        Ok(self
            .snapshot
            .classify_pair(self.key, &self.owner.terminal_id)?)
    }

    pub fn owner_tokens(&self) -> &'a Tokens {
        self.snapshot
            .pane_tokens(&self.owner.pane_id)
            .expect("verified owner tokens")
    }

    pub fn workspace_tokens(&self) -> &'a Tokens {
        self.snapshot
            .workspace_tokens(&self.caller.workspace_id)
            .expect("verified workspace tokens")
    }

    pub fn bound(&self) -> Result<&'a str> {
        let tokens = self.workspace_tokens();
        if self
            .binding
            .iter()
            .any(|(key, value)| tokens.get(key) != Some(value))
        {
            return Err(invalid("verified native Stream binding required"));
        }
        tokens
            .get("strm_binding")
            .filter(|value| !value.is_empty())
            .map(String::as_str)
            .ok_or_else(|| invalid("verified native binding lifetime required"))
    }

    pub fn validate_pair(&self) -> Result<()> {
        match self.pair()? {
            PairState::Empty => {}
            PairState::CardOnly { card, .. } => {
                if self
                    .snapshot
                    .pane_tokens(&card.pane.id)
                    .and_then(|tokens| tokens.get("strm_native"))
                    .map(String::as_str)
                    != Some(self.native_id)
                {
                    return Err(invalid("reserved card native Pi identity mismatch"));
                }
            }
            PairState::Absent { agent, .. }
            | PairState::Paired { agent, .. }
            | PairState::Separated { agent, .. } => {
                if NativeTuple::from_pane(agent) != *self.owner
                    || self.owner_tokens().get("strm_native").map(String::as_str)
                        != Some(self.native_id)
                {
                    return Err(invalid("native Pi identity or owner tuple mismatch"));
                }
            }
        }
        Ok(())
    }

    pub fn recorded_live(&self, attempt: &NativeAttempt) -> bool {
        attempt.resources().is_some_and(|resource| {
            self.snapshot.topology().panes.iter().any(|pane| {
                pane.terminal_id == resource.terminal_id()
                    || matches!(attempt.phase(), NativePhase::Reserved(_))
                        && pane.pane.id == resource.pane_id()
            })
        })
    }

    pub fn validate_evidence(
        &self,
        attempt: Option<NativeAttempt>,
    ) -> Result<Option<NativeAttempt>> {
        let pair = self.pair()?;
        if matches!(pair, PairState::CardOnly { .. }) {
            return Err(invalid(
                "uncertain card-only launch retained; owner must complete its exact reservation",
            ));
        }
        if let Some(record) = &attempt {
            if matches!(record.phase(), NativePhase::Unlaunched) {
                return Err(invalid(
                    "uncertain native launch retained; no retry permitted",
                ));
            }
            if matches!(record.phase(), NativePhase::Reserved(_)) {
                let resources = record.resources().expect("reserved resources");
                let mut derived = record.clone();
                match pair {
                    PairState::Absent { nonce, .. }
                        if nonce == record.nonce()
                            && self.owner_tokens().get("strm_card").map(String::as_str)
                                == Some(resources.terminal_id())
                            && !self.recorded_live(record) =>
                    {
                        derived.mark_closed()?
                    }
                    PairState::Paired { card, nonce, .. }
                        if nonce == record.nonce()
                            && NativeTuple::from_pane(card).matches_resources(resources) =>
                    {
                        derived.mark_ready()?
                    }
                    _ => {
                        return Err(invalid(
                            "uncertain reserved launch retained; exact pair proof required",
                        ));
                    }
                }
                return Ok(Some(derived));
            }
            if matches!(pair, PairState::Empty) && self.recorded_live(record) {
                return Err(invalid(
                    "recorded card is live without verified ownership; no duplicate or cleanup permitted",
                ));
            }
        }
        let matches = match pair {
            PairState::Empty => true,
            PairState::Absent { nonce, .. } => attempt.as_ref().is_some_and(|record| {
                record.nonce() == nonce
                    && record.resources().is_some_and(|resource| {
                        self.owner_tokens().get("strm_card").map(String::as_str)
                            == Some(resource.terminal_id())
                    })
            }),
            PairState::Paired { card, nonce, .. } | PairState::Separated { card, nonce, .. } => {
                attempt.as_ref().is_some_and(|record| {
                    record.nonce() == nonce
                        && record.resources().is_some_and(|resource| {
                            card.terminal_id == resource.terminal_id()
                                && self.owner_tokens().get("strm_card").map(String::as_str)
                                    == Some(resource.terminal_id())
                        })
                })
            }
            PairState::CardOnly { .. } => false,
        };
        if !matches {
            return Err(invalid(
                "pair does not match durable native launch evidence",
            ));
        }
        Ok(attempt)
    }
}
