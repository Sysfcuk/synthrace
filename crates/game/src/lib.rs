//! SYNTHRACE game crate: platform-independent scene building and VFX plus the browser entry point.

pub mod scene;
pub mod vfx;

#[cfg(target_arch = "wasm32")]
mod audio;
#[cfg(target_arch = "wasm32")]
mod input;
#[cfg(target_arch = "wasm32")]
mod web;
