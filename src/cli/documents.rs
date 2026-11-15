//! Current-file hints share the application's document policy.

use serde_json::Value;
use streams::{
    model::{Session, Stream},
    runtime::{CheckoutRegistry, DirectoryBindings},
};

pub(super) fn context(
    registry: &CheckoutRegistry,
    locations: &DirectoryBindings,
    stream: &Stream,
    session: &Session,
) -> Value {
    streams::application::documents::context(registry, locations, stream, session)
}
