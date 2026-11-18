//! Built-in connections behind the frontend interface, with no saved native identity.
use super::{Result, invalid};
use crate::{
    frontend::{FrontendId, WorkspaceFrontend, cmux::CmuxClient, herdr::HerdrClient},
    process::ProcessRunner,
};
use std::{collections::BTreeMap, sync::Arc};

/// Built-in connection policy, not a replacement for the extensible frontend trait.
#[derive(Clone)]
pub enum BuiltinFrontend {
    Herdr(HerdrClient),
    Cmux(CmuxClient),
}

impl BuiltinFrontend {
    pub fn connect(
        id: &FrontendId,
        runner: Arc<dyn ProcessRunner>,
        environment: BTreeMap<String, String>,
        timeout: f64,
    ) -> Result<Self> {
        match id {
            FrontendId::Herdr => Ok(Self::Herdr(
                HerdrClient::connect(environment, timeout)?.observing()?,
            )),
            FrontendId::Cmux => Ok(Self::Cmux(CmuxClient::connect(
                runner,
                environment,
                timeout,
            )?)),
            FrontendId::Hc => Err(invalid(format!("unsupported native frontend: {id}"))),
        }
    }

    pub fn herdr(&self) -> Option<&HerdrClient> {
        match self {
            Self::Herdr(client) => Some(client),
            Self::Cmux(_) => None,
        }
    }

    pub fn adapter(&self) -> &dyn WorkspaceFrontend {
        match self {
            Self::Herdr(client) => client,
            Self::Cmux(client) => client,
        }
    }
}
