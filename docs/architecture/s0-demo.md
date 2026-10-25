# First visible site — tor S0

The first runnable slice is intentionally limited to public **demo catalog + book detail + HTML/CSS**. Based on audited BookaRust source commit `d11b27b43125b93e59809147e8e687203fa0753b`; copied BookKey/Book/Visibility domain logic, public catalog use-case and HTML/CSS/templates. Reorganized into TOR feature-first crates.

Build/run locally with `cargo test --workspace`, `cargo build -p tor-web --release`, then `TOR_DEMO_MODE=1 TOR_LISTEN_ADDR=127.0.0.1:3000 ./target/release/tor-web`. The demo refuses to bind to a non-loopback interface. Serve externally only with separate nginx/TLS/auth gate.

Paths:
- `/`, `/catalog`, `/books/{id}` — real HTML (demo books).
- `/api/v1/books` — bounded public JSON listing.
- `/healthz`, `/readyz`, `/assets/site.css`.
- `/login`, `/register` — explicit disabled pages; no password/PII collection. POST methods rejected.
- Draft books always filtered in service and repository. Chapter texts, saved reading progress, payments and full accounts are **not** implemented by this demo.

Data is immutable preseeded examples in `Arc<Vec<Book>>` and resets after any restart. This is NOT 100% NEwBOO parity, nor actual PostgreSQL deployment. We are deliberately not copying global crates/domain/application/adapters from BookaRust: all new code follows `docs/architecture/feature-first.md`.

GitHub Actions builds release outside the small VPS and uploads the binary. First server activation remains manual behind proven SSH host identity, system nginx configuration and HTTPS. Before enabling automatic push-main deploy, verify rollback, locks, sufficient memory and real host.
