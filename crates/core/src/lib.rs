//! commons-core: the whole client (identity, MLS, sync, local history) without any UI.
pub mod call;
pub mod callbridge;
pub mod client;
pub mod envelope;
pub mod mls;
pub mod state;
pub mod transport;
pub mod vault;
mod ops;
mod sync;
pub mod api;
