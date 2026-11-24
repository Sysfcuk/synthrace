//! SYNTHRACE game crate: platform-independent scene building plus the browser entry point.

pub mod scene;

#[cfg(target_arch = "wasm32")]
mod audio;
#[cfg(target_arch = "wasm32")]
mod web;
