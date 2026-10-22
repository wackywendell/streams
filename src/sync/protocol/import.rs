//! Snapshot imports use the operation-save machine's existing Saving fence.

use super::{Action, ProtocolMachine};
use crate::sync::{adoption::PreparedImport, state::RepairRequired};

/// Owns validated preparation evidence and exclusively borrows its originating machine.
/// There is no raw import dispatch or second persistence state machine.
pub(crate) struct ImportPermit<'machine, 'catalog> {
    machine: &'machine mut ProtocolMachine,
    prepared: PreparedImport<'catalog>,
}

impl ProtocolMachine {
    pub(crate) fn begin_import<'machine, 'catalog>(
        &'machine mut self,
        prepared: PreparedImport<'catalog>,
    ) -> Result<ImportPermit<'machine, 'catalog>, RepairRequired> {
        self.step(Action::BeginSave)?;
        Ok(ImportPermit {
            machine: self,
            prepared,
        })
    }
}

impl ImportPermit<'_, '_> {
    pub(crate) fn run(self) -> Result<String, RepairRequired> {
        match self.prepared.install() {
            Ok(tip) => {
                self.machine.step(Action::CompleteSave)?;
                Ok(tip)
            }
            Err(error) => {
                self.machine.step(Action::FailSaveNeedsRepair)?;
                Err(error)
            }
        }
    }
}
