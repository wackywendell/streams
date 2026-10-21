use super::*;

#[derive(PartialEq, Eq)]
struct RetirementProof {
    scope: Scope,
    owner: NativeTuple,
    markers: Tokens,
    binding: String,
    old: NativeAttempt,
    current: Option<NativeAttempt>,
    inventory: HandoverInventory,
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
        let inventory = observed.saved.handover.as_ref().ok_or_else(|| {
            self.refuse("fresh native inventory is required for predecessor retirement")
        })?;
        let mut candidates = Vec::new();
        for scope in &inventory.scopes {
            if scope.harness == "pi"
                && scope.native_session_id == *old_native
                && scope.catalog_source == current_scope.catalog_source
                && scope.host_id == current_scope.host_id
                && scope.stream_id == current_scope.stream_id
                && json_digest(&ScopeWire::from(scope))? == markers["strm_agent"]
            {
                candidates.push(scope);
            }
        }
        let [previous] = candidates.as_slice() else {
            return Err(self.refuse("predecessor catalog scope is not unique"));
        };
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
        let old_key = attempt_key(&self.client.endpoint().socket, previous, &observed.owner)?;
        let old = self
            .state
            .read(&old_key)?
            .ok_or_else(|| self.refuse("predecessor has no durable proof"))?;
        let Some(resources) = old.resources() else {
            return Err(self.refuse("uncertain predecessor launch retained"));
        };
        if !matches!(old.phase(), NativePhase::Ready(_) | NativePhase::Closed(_))
            || old.nonce() != markers["strm_nonce"]
            || resources.terminal_id() != markers["strm_card"]
            || resources.tab_id() != observed.owner.tab_id
            || resource_live(&observed, resources)
        {
            return Err(self.refuse("predecessor launch is pending, mismatched or still live"));
        }
        let current = self.state.read(&observed.key)?;
        if current.as_ref().is_some_and(|record| {
            matches!(
                record.phase(),
                NativePhase::Unlaunched | NativePhase::Reserved(_)
            ) || record
                .resources()
                .is_some_and(|resources| resource_live(&observed, resources))
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
        let _lock = state.lock(
            &socket,
            &self.caller.workspace_id,
            self.config.timeout_seconds,
            &self.cancelled,
        )?;
        let (_, before) = self.retirement_proof()?;
        let (_, fresh) = self.retirement_proof()?;
        if fresh != before {
            return Err(self.refuse("predecessor proof changed before retirement"));
        }
        let mut args = argv(&[
            "pane",
            "report-metadata",
            &fresh.owner.pane_id,
            "--source",
            PLUGIN_ID,
        ]);
        for key in PAIR_KEYS {
            args.extend(["--clear-token".into(), key.into()]);
        }
        self.call(&args, None, true)?;
        let after = self.inspect(true)?;
        let inventory = after
            .saved
            .handover
            .as_ref()
            .ok_or_else(|| self.refuse("retirement inventory unavailable"))?;
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

fn resource_live(proof: &Verified, resources: &Resources) -> bool {
    proof.snapshot.topology().panes.iter().any(|pane| {
        pane.pane.id == resources.pane_id() || pane.terminal_id == resources.terminal_id()
    })
}
fn validate_inventory(
    proof: &Verified,
    inventory: &HandoverInventory,
    old_native: &str,
    socket: &Path,
) -> Result<()> {
    if inventory.native.socket() != socket
        || inventory.native.panes().len() != proof.snapshot.topology().panes.len()
    {
        return Err(invalid(
            "native retirement inventory is incomplete or from another endpoint",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for row in inventory.native.panes() {
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
