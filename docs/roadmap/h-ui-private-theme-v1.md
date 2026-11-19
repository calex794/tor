# UI-03A — ручная тема в личной библиотеке без JavaScript

Status: Draft PR, not merged.

## Current constraint
- Account HTTP responses use Content-Security-Policy with \`script-src 'none'\` via \`auth_cookies::private_response\`.
- UI-01's first-paint script works on public pages but cannot apply \`localStorage\` in /me/library, where executing JavaScript is intentionally prohibited.

## Proposed safe implementation
- Public appearance script mirrors only validated system/light/dark theme enum into a first-party SameSite=Lax cookie \`tor-ui-theme-v1\`. The cookie has path /, no account identifiers, 1-year max age and Secure in HTTPS.
- On GET /me/library, the server reads that cookie from existing request headers, exact-matches the enum, and adds a CSS data attribute to its HTML root. It removes the inapplicable appearance script from the private shell.
- No session/CSRF cookie changes; no S3, PostgreSQL, payment, authorization, or new routes. The private CSP remains unchanged.
- Existing library layout and links remain identical. No-JavaScript and absent-cookie fallback use the OS theme.

## Known limitation
- Already-saved localStorage preferences reach the cookie only when the user visits a JS-enabled public page after this release.
- This step does not implement cross-device synchronization or a private-page picker. Those require separate approval and tests.
- Do not weaken the private CSP just to operate a theme switcher.

## Verification
- Rust unit tests: allowed and hostile cookie values; HTML escaping boundary; no appearance script in private HTML.
- Node test: first-paint cookie mirror and Secure behavior on HTTP/HTTPS.
- Full CI including authenticated PostgreSQL and Chromium regression is required before merging.
