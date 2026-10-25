//! Record-local agent capture and typed intent; no inferred attachment or refreshed base.

mod capture;
mod enrollment;
mod preview;
mod record_context;
pub(crate) mod record_update;

pub use capture::{CaptureTarget, capture_inputs};
pub use enrollment::{EnrollmentRequest, StreamProposal, effort_id_for};
pub use preview::preview;
pub use record_context::{
    ContextRequest, RecordContext, RecordContextError, record_context, session_id_for,
};
pub use record_update::{RecordUpdate, parse_record_update};
