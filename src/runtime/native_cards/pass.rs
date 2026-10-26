use super::{closing::ClosePlan, linking::LinkPlan, *};
use crate::agent::RecordContext;
use serde_json::json;

/// Execution capabilities. The driver acquires the guard before its one snapshot.
pub struct NativePorts<'a> {
    pub client: &'a HerdrClient,
    pub writer: &'a NativeStateGuard<'a>,
    pub cancelled: &'a AtomicBool,
}

/// One owner-triggered pass over already-captured catalog and native inputs.
/// It never resolves an owner, obtains a snapshot or calls a catalog/Source adapter.
pub struct NativePass<'a> {
    records: &'a RecordContext,
    membership: NativeMembershipObservation<'a>,
    ports: NativePorts<'a>,
    scope: Scope,
    owner: NativeTuple,
    key: String,
    binding: Tokens,
    attempt: Option<NativeAttempt>,
    commands: Vec<CommandEvidence>,
}

impl<'a> NativePass<'a> {
    pub fn new(
        records: &'a RecordContext,
        membership: NativeMembershipObservation<'a>,
        ports: NativePorts<'a>,
    ) -> Result<Self> {
        if ports.cancelled.load(Ordering::Acquire) {
            return Err(invalid("native operation cancelled"));
        }
        let scope = membership.admit_context(records)?;
        let caller = membership.caller().caller();
        if caller.endpoint() != ports.client.endpoint() {
            return Err(invalid(
                "native pass endpoint differs from its caller observation",
            ));
        }
        let owner = NativeTuple::from_pane(
            membership
                .caller()
                .snapshot()
                .locate_terminal(caller.terminal_id())?,
        );
        let key = conversation_key(&scope)?;
        let binding = BTreeMap::from([
            ("strm_catalog".into(), json_digest(&scope.catalog_source)?),
            ("strm_host".into(), scope.host_id.clone()),
            ("strm_stream".into(), scope.stream_id.clone()),
        ]);
        let attempt = ports.writer.read(&key)?;
        if attempt.as_ref().is_some_and(|record| {
            record.endpoint() != ports.client.endpoint().socket
                || record.purpose() != NativePurpose::Card
        }) {
            return Err(invalid(
                "native pass evidence endpoint or purpose changed; fence retained",
            ));
        }
        let pass = Self {
            records,
            membership,
            ports,
            scope,
            owner,
            key,
            binding,
            attempt,
            commands: Vec::new(),
        };
        pass.facts().validate_pair()?;
        Ok(pass)
    }

    pub fn records(&self) -> &RecordContext {
        self.records
    }

    pub fn caller(&self) -> &super::super::native::PiCallerObservation {
        self.membership.caller()
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    fn facts(&self) -> NativeFacts<'_> {
        NativeFacts {
            snapshot: self.membership.caller().snapshot(),
            caller: &self.owner,
            owner: &self.owner,
            key: &self.key,
            native_id: &self.scope.native_session_id,
            binding: &self.binding,
        }
    }

    /// Consume the pass so later effects cannot reuse its pre-effect snapshot.
    pub fn close_display(mut self) -> Result<()> {
        let record = self
            .attempt
            .as_ref()
            .ok_or_else(|| invalid("missing close evidence"))?;
        self.facts().validate_evidence(Some(record.clone()))?;
        let plan = ClosePlan::from_observation(&self.facts(), record)?;
        self.check_cancelled()?;
        if let Some(closed) = plan.closed {
            self.ports.writer.write(Some(record), &closed)?;
        }
        let Some(card) = plan.card else { return Ok(()) };
        let reply = self.call(
            "plugin.pane.close",
            json!({"pane_id":card.pane_id}),
            "plugin_pane_closed",
        )?;
        if reply["pane_id"] != card.pane_id {
            return Err(
                self.refuse("close acknowledgement changed; closed intent and markers retained")
            );
        }
        Ok(())
    }

    pub fn link_reserved(mut self) -> Result<Resources> {
        let record = self
            .attempt
            .clone()
            .ok_or_else(|| invalid("missing reserved evidence"))?;
        let plan = LinkPlan::from_observation(&self.facts(), &record)?;
        for (pane, tokens) in plan.updates {
            self.call(
                "pane.report_metadata",
                json!({"pane_id":pane,"source":PLUGIN_ID,"tokens":tokens}),
                "ok",
            )?;
        }
        self.check_cancelled()?;
        self.ports.writer.write(Some(&record), &plan.ready)?;
        plan.card.resources()
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.ports.cancelled.load(Ordering::Acquire) {
            Err(invalid("native operation cancelled"))
        } else {
            Ok(())
        }
    }

    fn refuse(&self, message: impl Into<String>) -> super::super::RuntimeError {
        FrontendError {
            message: message.into(),
            commands: self.commands.clone(),
        }
        .into()
    }

    fn call(&mut self, method: &str, params: Value, expected: &str) -> Result<Value> {
        self.check_cancelled()?;
        let (value, commands) = self
            .ports
            .client
            .card_request(method, params, Some(expected), true)
            .map_err(|mut error| {
                let mut prior = self.commands.clone();
                prior.append(&mut error.commands);
                error.commands = prior;
                error
            })?;
        self.commands.extend(commands);
        Ok(value)
    }
}
