//! astrid-tui — the only surface. ratatui transcript with receipt cards,
//! approval flow, and a keyboard-driven input loop.

pub mod app;
pub mod bridge;

pub use app::{run_app, TuiStatus};
pub use bridge::ApprovalBridge;
