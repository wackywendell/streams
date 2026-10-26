use super::*;
use serde_json::json;

pub(super) struct ClosePlan {
    pub card: Option<NativeTuple>,
    pub closed: Option<NativeAttempt>,
}

impl ClosePlan {
    pub(super) fn from_observation(v: &NativeFacts<'_>, record: &NativeAttempt) -> Result<Self> {
        v.bound()?;
        let card = match v.pair()? {
            PairState::Empty | PairState::Absent { .. } => None,
            PairState::Paired { card, .. } => Some(NativeTuple::from_pane(card)),
            _ => return Err(invalid("verified paired or absent owner markers required")),
        };
        let closed = if matches!(record.phase(), NativePhase::Closed(_)) {
            None
        } else {
            let mut closed = record.clone();
            closed.mark_closed()?;
            Some(closed)
        };
        Ok(Self { card, closed })
    }
}

impl NativeCardEngine {
    pub fn close_display(&mut self) -> Result<()> {
        self.begin_observation();
        let state = Arc::clone(&self.state);
        let lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
        let observed = self.guard()?;
        let record = self
            .read_attempt(&observed.key, NativePurpose::Card)?
            .ok_or_else(|| self.refuse("missing close evidence"))?;
        self.validate_evidence(&observed, Some(record.clone()))?;
        let plan = ClosePlan::from_observation(&observed.facts(), &record)?;
        self.check_cancelled()?;
        if let Some(closed) = plan.closed {
            lock.write(Some(&record), &closed)?;
        }
        let Some(card) = plan.card else {
            return Ok(());
        };
        let reply = self.call(
            "plugin.pane.close",
            json!({"pane_id":card.pane_id}),
            Some("plugin_pane_closed"),
            true,
        )?;
        if reply["pane_id"] != card.pane_id {
            return Err(
                self.refuse("close acknowledgement changed; closed intent and markers retained")
            );
        }
        Ok(())
    }
}
