# MAIL-01b1 — DNS routing decision

Line A. Public email transport domains are classified before any future SMTP submission. RFC 5321 section 5.1 and RFC 7505 govern explicit/implicit MX and Null MX. No MX record (NODATA) triggers A/AAAA checks; authoritative NXDOMAIN is distinct from timeouts, REFUSED, SERVFAIL and malformed responses. A single preference-zero root exchange is a Null MX; mixed/malformed RRsets are inconclusive. An explicit MX is an advertised route, not proof that a particular mailbox or MX host is reachable. Contradictory DNS evidence defers rather than rejecting permanently.

The injected resolver receives only an ASCII domain; never the recipient local part. At most three lookups, 2 seconds per lookup, A/AAAA concurrent. The decision carries DNS TTL for a later bounded cache. Inconclusive outcomes must not trigger SMTP or a permanent suppression entry.

This PR introduces the executable decision service and fake-resolver tests, not the live resolver adapter. The production adapter will use a maintained DNS library and the operator's trusted recursive resolver, preserve RCODE/negative TTL and use absolute queries; no ad hoc DNS packet parser, remote mailbox probing or dependency on commercial validation APIs. MAIL-01b2 must prove real DNS fixture behavior including CNAME, NXDOMAIN, NODATA, SERVFAIL and timeouts before integration. MAIL-01c will add bounded TTL cache, coalescing and global concurrency limits. No change to registration or authentication and no deployment.

Acceptance: cargo test -p tor-accounts services::recipient_dns; normal CI remains mandatory. Tests are deterministic and do not contact public DNS or SMTP.
