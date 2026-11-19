use super::*;
use streams::process::NativeProcessRunner;

impl App {
    pub(super) fn checkout(&self, command: CheckoutCommand) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let registry = opened.registry(&self.config)?;
        let value = match command {
            CheckoutCommand::List => json!({"checkouts":registry.list()?}),
            CheckoutCommand::Register {
                directory,
                repository,
            } => {
                json!({"checkout":registry.register(&self.path(&directory)?,repository.as_deref())?})
            }
            CheckoutCommand::Show { checkout_id } => {
                json!({"checkout":registry.get(&checkout_id)?})
            }
            CheckoutCommand::Document {
                checkout_id,
                path,
                revision,
            } => {
                let cache_root = opened.runtime_state(&self.config)?.root().join("revisions");
                serde_json::to_value(registry.revision_document(
                    &checkout_id,
                    &path,
                    &revision,
                    &NativeProcessRunner,
                    30.0,
                    &cache_root,
                )?)?
            }
            CheckoutCommand::Move {
                checkout_id,
                directory,
            } => {
                json!({"checkout":registry.move_checkout(&checkout_id,&self.path(&directory)?)?})
            }
            CheckoutCommand::Default {
                repository,
                checkout_id,
            } => {
                registry.set_default(&repository, &checkout_id)?;
                json!({"repository":repository,"checkout_id":checkout_id})
            }
            CheckoutCommand::Resolve { repository } => {
                json!({"checkout":registry.resolve(&repository,None,None)?})
            }
        };
        Ok(Output::value(value))
    }
}
