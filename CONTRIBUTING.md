# Contributing

`fingerprint-kit` is a small, experimental reference implementation. Focused bug fixes,
documentation improvements, tests, and changes within the documented hardware-free scope are
welcome. Open an issue before substantial design or scope changes.

## Security and biometric data

- Never submit real fingerprint captures, biometric templates, credentials, or identifying test
  fixtures.
- Report suspected vulnerabilities through
  [private vulnerability reporting](https://github.com/P4suta/fingerprint-kit/security/advisories/new),
  not a public issue.
- Preserve the redaction and bounded-input guarantees described in [SECURITY.md](SECURITY.md).

## Development checks

Rust 1.97.1 is selected by `rust-toolchain.toml`. Before opening a pull request, run:

```console
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo doc --locked --no-deps
cargo run --locked -- demo
```

Set `RUSTDOCFLAGS="-D warnings"` for the documentation build. Keep commits focused, update tests and
documentation with behavior changes, and explain dependency updates or security-policy changes in
the pull request.
