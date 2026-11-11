//! Aggregate-only FLOW signals. Numerical policy is an implementation default.
mod evaluate;
mod state;
mod types;

pub use evaluate::evaluate;
pub use state::{transition, Episode, EventKind, Transition, COOLDOWN_MS};
pub use types::*;

#[cfg(test)]
mod tests;
