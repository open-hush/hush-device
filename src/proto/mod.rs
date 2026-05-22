//! Wire types and inter-task event types.
//!
//! - [`api`] mirrors the request/response schemas defined in
//!   `hush-protocol/hush-api.yaml`. **Keep them in lockstep with the spec.**
//! - [`events`] defines the broadcast event enum used by the `embassy_sync`
//!   pubsub channel.

pub mod api;
pub mod events;
pub mod led;
