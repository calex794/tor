//! Fixed MatchedPath taxonomy: immutable route IDs, no per-request allocation.
use super::*;

mod classifier;
mod entries_core;
mod entries_layer;
pub use classifier::classify;

use entries_core::BASE_ROUTES;
use entries_layer::LAYER_ROUTES;

const ROUTES: [RouteRegistration; 87] = {
    let mut result = [BASE_ROUTES[0]; 87];
    let mut i = 0;
    while i < 78 {
        result[i] = BASE_ROUTES[i];
        i += 1;
    }
    let mut j = 0;
    while j < 9 {
        result[78 + j] = LAYER_ROUTES[j];
        j += 1;
    }
    result
};

/// Read-only, finite route graph for governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}
