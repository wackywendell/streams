use super::*;
use serde_json::json;

struct LinkPlan {
    card: NativeTuple,
    updates: Vec<(String, Tokens)>,
    ready: NativeAttempt,
}

impl LinkPlan {
    fn from_observation(v: &Verified, attempt: &NativeAttempt) -> Result<Self> {
        v.bound()?;
        if v.caller != v.owner
            || attempt.key() != v.key
            || attempt.purpose() != NativePurpose::Card
            || !matches!(attempt.phase(), NativePhase::Reserved(_))
        {
            return Err(invalid("reserved identity or durable evidence changed"));
        }
        let receipt = attempt.resources().expect("reserved resources");
        let pane = v
            .snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| pane.pane.id == receipt.pane_id())
            .ok_or_else(|| invalid("reserved card missing; no relaunch permitted"))?;
        let card = NativeTuple::from_pane(pane);
        if !card.matches_resources(receipt)
            || card.workspace_id != v.owner.workspace_id
            || card.tab_id != v.owner.tab_id
        {
            return Err(invalid("reserved card tuple moved or changed"));
        }
        match v.pair()? {
            PairState::Empty => {
                if PAIR_KEYS.iter().any(|key| {
                    v.snapshot
                        .pane_tokens(&card.pane_id)
                        .is_some_and(|tokens| tokens.contains_key(*key))
                }) {
                    return Err(invalid("incomplete or conflicting reserved pair markers"));
                }
            }
            PairState::CardOnly {
                card: paired,
                nonce,
            }
            | PairState::Paired {
                card: paired,
                nonce,
                ..
            } if nonce == attempt.nonce() && NativeTuple::from_pane(paired) == card => {}
            _ => return Err(invalid("conflicting reserved pair publisher")),
        }
        let mut desired: Tokens = BTreeMap::from([
            ("strm_agent".into(), v.identity.clone()),
            ("strm_nonce".into(), attempt.nonce().into()),
            ("strm_pi".into(), v.owner.terminal_id.clone()),
            ("strm_card".into(), card.terminal_id.clone()),
            (
                "strm_native".into(),
                v.saved.card.scope().native_session_id.clone(),
            ),
            ("strm_role".into(), "card".into()),
        ]);
        let mut updates = Vec::new();
        for id in [&card.pane_id, &v.owner.pane_id] {
            let observed = v.snapshot.pane_tokens(id).expect("verified pane tokens");
            if desired
                .iter()
                .any(|(key, value)| observed.get(key).is_some_and(|observed| observed != value))
            {
                return Err(invalid("conflicting reserved ownership markers"));
            }
            let changed: Tokens = desired
                .iter()
                .filter(|(key, value)| observed.get(*key) != Some(*value))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !changed.is_empty() {
                updates.push((id.clone(), changed));
            }
            desired.insert("strm_role".into(), "agent".into());
        }
        let mut ready = attempt.clone();
        ready.mark_ready()?;
        Ok(Self {
            card,
            updates,
            ready,
        })
    }
}

impl NativeCardEngine {
    pub(super) fn link_card(
        &mut self,
        lock: &NativeStateGuard<'_>,
        observed: &Verified,
        attempt: &NativeAttempt,
    ) -> Result<Resources> {
        let plan = LinkPlan::from_observation(observed, attempt)?;
        for (pane, tokens) in plan.updates {
            self.call(
                "pane.report_metadata",
                json!({"pane_id":pane,"source":PLUGIN_ID,"tokens":tokens}),
                Some("ok"),
                true,
            )?;
        }
        self.check_cancelled()?;
        lock.write(Some(attempt), &plan.ready)?;
        plan.card.resources()
    }
}
