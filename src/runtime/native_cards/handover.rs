use super::*;

#[derive(PartialEq, Eq)]
struct RetirementProof {
    scope: Scope,
    owner: NativeTuple,
    markers: Tokens,
    binding: String,
    old: NativeAttempt,
    current: Option<NativeAttempt>,
    inventory: NativeInventory,
}

impl NativeCardEngine {
    pub(super) fn guard_recovering(&mut self) -> Result<Verified> {
        let observed = self.inspect(false)?;
        if observed
            .owner_tokens()
            .get("strm_native")
            .is_some_and(|old| old != &observed.saved.card.scope().native_session_id)
        {
            self.retire_predecessor()?;
            self.guard()
        } else {
            self.validate_pair(observed)
        }
    }

    fn retirement_proof(&mut self) -> Result<(Verified, RetirementProof)> {
        let observed = self.inspect(true)?;
        if observed.caller != observed.owner {
            return Err(self.refuse("only owning Pi may retire a predecessor"));
        }
        let markers = observed.owner_tokens();
        if PAIR_KEYS
            .iter()
            .any(|key| markers.get(*key).is_none_or(String::is_empty))
            || markers.get("strm_role").map(String::as_str) != Some("agent")
            || markers.get("strm_pi") != Some(&observed.owner.terminal_id)
        {
            return Err(self.refuse("predecessor markers are incomplete or foreign"));
        }
        let old_native = &markers["strm_native"];
        let current_scope = observed.saved.card.scope();
        if old_native == &current_scope.native_session_id {
            return Err(self.refuse("predecessor is current conversation"));
        }
        let inventory = &observed
            .saved
            .handover
            .as_ref()
            .ok_or_else(|| {
                self.refuse("fresh native inventory is required for predecessor retirement")
            })?
            .native;
        let old_key = conversation_key_for(
            &current_scope.catalog_source,
            &current_scope.host_id,
            &current_scope.harness,
            old_native,
        )?;
        if old_key != markers["strm_agent"] {
            return Err(self.refuse("predecessor conversation principal differs"));
        }
        let PairState::Absent { agent, .. } = observed
            .snapshot
            .classify_pair(&markers["strm_agent"], &observed.owner.terminal_id)?
        else {
            return Err(
                self.refuse("predecessor card must be absent; no live card is adopted or closed")
            );
        };
        if NativeTuple::from_pane(agent) != observed.owner {
            return Err(self.refuse("predecessor owner tuple differs"));
        }
        validate_inventory(
            &observed,
            inventory,
            old_native,
            &self.client.endpoint().socket,
        )?;
        if observed.snapshot.topology().panes.iter().any(|pane| {
            pane.pane.id != observed.owner.pane_id
                && observed
                    .snapshot
                    .pane_tokens(&pane.pane.id)
                    .and_then(|tokens| tokens.get("strm_native"))
                    == Some(old_native)
        }) {
            return Err(self.refuse("predecessor has live native markers"));
        }
        let old = self
            .read_attempt(&old_key, NativePurpose::Card)?
            .ok_or_else(|| self.refuse("predecessor has no durable proof"))?;
        let Some(resources) = old.resources() else {
            return Err(self.refuse("uncertain predecessor launch retained"));
        };
        if !matches!(old.phase(), NativePhase::Ready(_) | NativePhase::Closed(_))
            || old.nonce() != markers["strm_nonce"]
            || resources.terminal_id() != markers["strm_card"]
            || super::actions::recorded_live(&observed, &old)
        {
            return Err(self.refuse("predecessor launch is pending, mismatched or still live"));
        }
        let current = self.read_attempt(&observed.key, NativePurpose::Card)?;
        if current.as_ref().is_some_and(|record| {
            matches!(
                record.phase(),
                NativePhase::Unlaunched | NativePhase::Reserved(_)
            ) || super::actions::recorded_live(&observed, record)
        }) {
            return Err(self.refuse("current conversation has unresolved native evidence"));
        }
        let proof = RetirementProof {
            scope: current_scope.clone(),
            owner: observed.owner.clone(),
            markers: markers.clone(),
            binding: observed.bound()?.into(),
            old,
            current,
            inventory: inventory.clone(),
        };
        Ok((observed, proof))
    }

    fn retire_predecessor(&mut self) -> Result<()> {
        let state = self.state.clone();
        let socket = self.client.endpoint().socket.clone();
        let _lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
        let (_, before) = self.retirement_proof()?;
        let (_, fresh) = self.retirement_proof()?;
        if fresh != before {
            return Err(self.refuse("predecessor proof changed before retirement"));
        }
        let tokens: BTreeMap<_, _> = PAIR_KEYS.iter().map(|key| (*key, Value::Null)).collect();
        let params = serde_json::json!({"pane_id":fresh.owner.pane_id,"source":PLUGIN_ID,
            "tokens":tokens});
        self.call("pane.report_metadata", params, None, true)?;
        let after = self.inspect(true)?;
        let inventory = &after
            .saved
            .handover
            .as_ref()
            .ok_or_else(|| self.refuse("retirement inventory unavailable"))?
            .native;
        validate_inventory(&after, inventory, &fresh.markers["strm_native"], &socket)?;
        if after.saved.card.scope() != &fresh.scope
            || after.owner != fresh.owner
            || after.bound()? != fresh.binding
            || after
                .owner_tokens()
                .keys()
                .any(|key| PAIR_KEYS.contains(&key.as_str()))
            || inventory != &fresh.inventory
            || self.state.read(fresh.old.key())?.as_ref() != Some(&fresh.old)
            || self.state.read(&after.key)? != fresh.current
        {
            return Err(self.refuse("retirement acknowledgement changed; effect may be partial"));
        }
        if !matches!(self.validate_pair(after)?.pair()?, PairState::Empty) {
            return Err(self.refuse("predecessor retirement did not establish an empty owner"));
        }
        Ok(())
    }
}

fn validate_inventory(
    proof: &Verified,
    inventory: &NativeInventory,
    old_native: &str,
    socket: &Path,
) -> Result<()> {
    if inventory.socket() != socket
        || inventory.panes().len() != proof.snapshot.topology().panes.len()
    {
        return Err(invalid(
            "native retirement inventory is incomplete or from another endpoint",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for row in inventory.panes() {
        let pane = proof.snapshot.locate_terminal(row.terminal_id())?;
        if !seen.insert(row.pane_id())
            || pane.pane.id != row.pane_id()
            || pane.pane.tab_id != row.tab_id()
            || pane.pane.workspace_id != row.workspace_id()
            || row.native_session_id() == Some(old_native)
            || (pane.pane.id == proof.owner.pane_id
                && row.native_session_id()
                    != Some(proof.saved.card.scope().native_session_id.as_str()))
        {
            return Err(invalid(
                "native retirement inventory changed or predecessor remains live",
            ));
        }
    }
    Ok(())
}
