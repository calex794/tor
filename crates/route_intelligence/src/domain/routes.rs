//! Fixed MatchedPath taxonomy: immutable route IDs, no per-request allocation.
use super::*;
use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Auth, Dashboard, Reading, System};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{PublicPage as Page, Static};

mod classifier;
mod continuation;
mod entries_account;
mod entries_core;
mod entries_follow;
mod entries_inbox;
mod entries_layer;
mod entries_mentions;
mod entries_referrals;
mod entries_reply;
pub use classifier::classify;

use entries_account::ACCOUNT_ROUTES;
use entries_core::BASE_ROUTES;
use entries_follow::FOLLOW_ROUTES;
use entries_inbox::INBOX_ROUTES;
use entries_layer::{LAYER_INDEX_ROUTES, LAYER_METADATA_ROUTES, LAYER_ROUTES};
use entries_mentions::MENTION_ROUTES;
use entries_referrals::REFERRAL_ROUTES;
use entries_reply::REPLY_ROUTES;

const COMPOSED_ROUTES: [RouteRegistration; 139] = {
    let mut result = [BASE_ROUTES[0]; 139];
    let mut i = 0;
    while i < 79 {
        result[i] = BASE_ROUTES[i];
        i += 1;
    }
    let mut j = 0;
    while j < 12 {
        result[79 + j] = LAYER_ROUTES[j];
        j += 1;
    }
    let mut k = 0;
    while k < 2 {
        result[91 + k] = FOLLOW_ROUTES[k];
        k += 1;
    }
    result[93] = REPLY_ROUTES[0];
    result[94] = LAYER_INDEX_ROUTES[0];
    result[95] = INBOX_ROUTES[0];
    result[96] = INBOX_ROUTES[1];
    result[97] = LAYER_METADATA_ROUTES[0];
    result[98] = RouteRegistration::new(
        "/me/updates/{id}/read-state",
        "POST",
        "/me/updates/",
        Prefix,
        Page,
        Reading,
        Write,
    );
    result[99] = RouteRegistration::new(
        "/me/settings/password",
        "GET",
        "/me/",
        Exact,
        Page,
        Auth,
        Read,
    );
    result[100] = RouteRegistration::new(
        "/me/settings/password",
        "POST",
        "/me/",
        Exact,
        Page,
        Auth,
        Write,
    );
    let mut n = 0;
    while n < 14 {
        result[101 + n] = ACCOUNT_ROUTES[n];
        n += 1;
    }
    let mut referral = 0;
    while referral < 9 {
        result[115 + referral] = REFERRAL_ROUTES[referral];
        referral += 1;
    }
    result[124] = MENTION_ROUTES[0];
    result[125] = MENTION_ROUTES[1];
    result[126] = RouteRegistration::new(
        "/me/books/{id}/chapters/import",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    );
    result[127] = RouteRegistration::new(
        "/me/books/{id}/chapters/import-preview",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    );
    result[128] = RouteRegistration::new(
        "/me/books/{id}/chapters/import-confirm",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    );
    result[129] = RouteRegistration::new(
        "/assets/newboo-chapter-import.js",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    );
    result[130] = RouteRegistration::new(
        "/books/{id}/stats",
        "GET",
        "/books/",
        Prefix,
        Page,
        Dashboard,
        Read,
    );
    result[131] = RouteRegistration::new(
        "/books/{id}/stats/export.csv",
        "GET",
        "/books/",
        Prefix,
        Page,
        Dashboard,
        Read,
    );
    result[132] = RouteRegistration::new(
        "/reader/{id}/thanks",
        "GET",
        "/reader/",
        Prefix,
        Page,
        Reading,
        Read,
    );
    result[133] = RouteRegistration::new(
        "/reader/{id}/thanks",
        "POST",
        "/reader/",
        Prefix,
        Page,
        Reading,
        Write,
    );
    result[134] =
        RouteRegistration::new("/me/creators", "GET", "/me/", Exact, Page, Dashboard, Read);
    result[135] = RouteRegistration::new(
        "/me/creators/{id}/unfollow",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    );
    result[136] = RouteRegistration::new(
        "/me/books/{id}/follow-creator",
        "POST",
        "/me/",
        Prefix,
        Page,
        Reading,
        Write,
    );
    result[137] = RouteRegistration::new(
        "/books/{id}/like",
        "GET",
        "/books/",
        Prefix,
        Page,
        Reading,
        Read,
    );
    result[138] = RouteRegistration::new(
        "/books/{id}/like",
        "POST",
        "/books/",
        Prefix,
        Page,
        Reading,
        Write,
    );
    result
};

// Keep every registered main route at its original stable numeric ID
// 0..124. Draft-only D-05 routes are appended 125..138. COMPOSED_ROUTES
// retains source declarations exactly once for RIL static auditing.
#[rustfmt::skip]
const MAIN_FIRST_ORDER: [usize; 139] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
    36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59,
    60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71,
    72, 73, 74, 75, 76, 77, 78, 91, 92, 93, 95, 96,
    98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109,
    110, 111, 112, 115, 116, 117, 118, 119, 120, 121, 122, 123,
    113, 114, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133,
    134, 135, 136, 137, 138, 79, 80, 81, 82, 83, 84, 85,
    86, 87, 88, 89, 90, 94, 97,
];
const ROUTES: [RouteRegistration; 139] = {
    let mut ordered = [COMPOSED_ROUTES[0]; 139];
    let mut i = 0;
    while i < 139 {
        ordered[i] = COMPOSED_ROUTES[MAIN_FIRST_ORDER[i]];
        i += 1;
    }
    ordered
};

/// Read-only, finite route graph for governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}
