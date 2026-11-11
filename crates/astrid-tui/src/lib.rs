//! astrid-tui — the only surface. Themed ratatui transcript with receipt
//! cards, streaming deltas, approval flow, and a keyboard-driven input loop.

pub mod app;
pub mod bridge;
pub mod theme;

pub use app::{run_app, TuiStatus};
pub use bridge::ApprovalBridge;
pub use theme::{by_name, Palette, THEMES};
