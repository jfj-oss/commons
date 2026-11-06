//! commons-core: the whole client (identity, MLS, sync, local history) without any UI.
pub mod client;
pub mod envelope;
pub mod mls;
pub mod state;
pub mod transport;
pub mod vault;
pub mod ops;
mod round;
mod sync;
pub mod api;
