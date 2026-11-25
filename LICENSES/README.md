# Third-party licenses

TOR's original code is subject to the root [ownership notice](../LICENSE).
That notice does not relicense dependencies, data, or third-party components.

The pinned `webpki-roots 1.0.12` dependency supplies TLS trust-anchor data under
CDLA-Permissive-2.0. The [license text](CDLA-Permissive-2.0.txt) is reproduced
from the SPDX license-list-data repository; the [official agreement](https://cdla.dev/permissive-2.0/)
is the authoritative reference. When distributing the data (including embedded
copies), make the agreement text available with them. Keep this text with any
externally distributed release containing the data. Re-review the exception
before upgrading the crate.

This directory is not a complete third-party notices inventory. Before any
external binary or source distribution, verify all applicable upstream license
and notice obligations against the exact locked dependency graph. A passing
cargo-deny check alone does not establish distribution compliance.
