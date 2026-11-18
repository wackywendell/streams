//! Built-in connections behind the frontend interface, with no saved native identity.
use crate::frontend::{WorkspaceFrontend, cmux::CmuxClient, herdr::HerdrClient};

/// Built-in connection policy, not a replacement for the extensible frontend trait.
#[derive(Clone)]
pub enum BuiltinFrontend {
    Herdr(HerdrClient),
    Cmux(CmuxClient),
}

impl BuiltinFrontend {
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
