//! Calls inside the app (CALLS-PLAN.md). Built only with the `calls` feature, so the rest of the app builds without
//! libwebrtc.
pub mod audio;
pub mod ice;
pub mod keys;
pub mod settings;
pub mod tone;
pub mod gate;
pub mod engine;
pub mod errors;
pub mod headless;
pub mod reconnect;
pub mod review;
pub mod testhook;
