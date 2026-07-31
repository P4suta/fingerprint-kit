## Summary

Describe what changed and why.

## Security and privacy

- [ ] No real biometric data, credentials, or sensitive fixtures are included.
- [ ] Pixel, minutiae, identifier, and attacker-controlled payloads remain redacted from logs and errors.
- [ ] Format, policy, or trust-boundary changes are documented in README, ARCHITECTURE, or SECURITY as appropriate.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --locked --all-targets --all-features -- -D warnings`
- [ ] `cargo test --locked --all-targets`
- [ ] `cargo doc --locked --no-deps` with `RUSTDOCFLAGS="-D warnings"`
- [ ] `cargo run --locked -- demo`
