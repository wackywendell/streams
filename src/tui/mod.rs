//! Pure work-card painting. Observations and returned targets confer no native authority.

mod render;
mod snapshot;
mod view;

pub use ratatui::{buffer::Buffer, layout::Rect};
pub use snapshot::{
    CardSnapshot, CheckoutBinding, CheckoutSelection, DisplayContext, DocumentLocation,
    DocumentProblem, DocumentTarget, EditorIdentity, NativeOwnerObservation, ResolvedDocument,
    Scope,
};
pub use view::{
    CapturedAction, CardView, FrameIdentity, Freshness, PaintedFrame, RefreshToken, Target,
    VisibleLink,
};

#[cfg(test)]
mod tests;
