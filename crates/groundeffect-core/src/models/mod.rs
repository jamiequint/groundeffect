//! Data models for GroundEffect
//!
//! Core data structures for emails, calendar events, accounts, and attachments.

mod account;
mod attachment;
mod calendar;
mod email;

pub use account::*;
pub use attachment::*;
pub use calendar::*;
pub use email::*;
