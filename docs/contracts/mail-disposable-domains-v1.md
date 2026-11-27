# MAIL-01d — disposable email domain policy

Source: https://github.com/disposable-email-domains/disposable-email-domains/tree/9cf2bee345b0276e1b65cee7484beb51e5c715b1 (CC0-1.0, upstream README license section). The exact 10,455-domain snapshot is vendored; upstream Git blob SHA fe0a192a919ca86081d9bd9b8330227cf5f8d55d. Upstream warns that the list may include domains no longer disposable. A match is a risk signal, not proof of abuse.

The classifier reuses MAIL-01a transport-address validation and IDNA conversion, matches complete domain labels and their parent suffixes (never a single TLD), and returns only a status. Exact reviewed local exceptions take precedence for that domain; child domains are not implicitly exempt. No runtime network, API, new dependencies or local-part retention. List parsing is bounded and initialized once per process; invalid data returns an explicit error.

Default Observe mode records a risk signal without blocking; Enforce rejects listed domains only. Registration/worker wiring, metrics, operator switch and enforcement activation are separate acceptance gates. Do not enable enforcement before reviewing observed false-positive rates. Existing accounts must not be prevented from login or accessing support.

## Reviewed updates

Download the exact file from an explicitly reviewed upstream commit (not a moving branch). Obtain its Git blob SHA from GitHub. Run `python3 tools/update_disposable_email_domains.py downloaded.conf --commit FULL_SHA --blob FULL_BLOB_SHA` for dry-run validation; add `--write` to prepare the snapshot and provenance metadata. The script rejects malformed, duplicate, unsorted, oversized, too-small and common-provider-blocking inputs. It does not change local exceptions. Review additions/removals in a PR, run `cargo test -p tor-accounts domain::email::disposable` and normal CI before accepting. A periodic reviewed update is preferred to silent fetching at runtime. Automated update scheduling and admin-managed exceptions are not implemented here.

Sources: upstream README and blocklist at the pinned commit; MAIL-01a policy. No production switch or sending in this PR.
