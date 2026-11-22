//! CRAB: Cross-model Relay for Agentic Building.
//!
//! The pure modules compile on every target, so `cargo test` runs natively.
//! The Worker and Durable Object code compiles only for wasm32.

pub mod auth;
pub mod files;
pub mod mcp;
pub mod model;
pub mod tools;

#[cfg(target_arch = "wasm32")]
mod relay;
#[cfg(target_arch = "wasm32")]
mod router;

#[cfg(target_arch = "wasm32")]
#[worker::event(fetch)]
async fn fetch(
    req: worker::Request,
    env: worker::Env,
    _ctx: worker::Context,
) -> worker::Result<worker::Response> {
    console_error_panic_hook::set_once();
    router::handle(req, env).await
}
