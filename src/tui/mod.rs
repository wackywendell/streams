//! Work-card painting and terminal application. Targets confer no native authority.

pub mod application;

mod render;
mod snapshot;
mod view;

pub use ratatui::{buffer::Buffer, layout::Rect};
pub use snapshot::{
    CardRecords, CardSnapshot, CheckoutBinding, CheckoutSelection, DisplayContext,
    DocumentLocation, DocumentProblem, DocumentTarget, EditorIdentity, NativeOwnerObservation,
    ResolvedDocument, Scope,
};
pub use view::{
    CapturedAction, CardView, FrameIdentity, Freshness, PaintedFrame, RefreshToken, Target,
    VisibleLink,
};

#[cfg(test)]
mod tests;
