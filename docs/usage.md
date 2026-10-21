# Usage and scope

fingerprint-kit is an experimental pipeline for capture replay, enrollment, template storage, and verification.
The hardware-free path uses the published fprint-mindtct and fprint-bozorth3 crates.
It does not provide OS login or production authentication.
Matching policies are uncalibrated and make no biometric accuracy or presentation attack detection claim.

## Hardware-free demonstration

The pinned Rust toolchain includes the formatter and Clippy.
Build and run with the locked dependency set:

```sh
cargo build --locked
cargo run --locked -- demo
```

The demo creates synthetic captures and removes its temporary artifacts on completion.
To keep a manual round trip, run:

```sh
cargo run --locked -- synthetic --identity 41 --impression 0 --out capture-0
cargo run --locked -- synthetic --identity 41 --impression 1 --out capture-1
cargo run --locked -- synthetic --identity 41 --impression 2 --out capture-2
cargo run --locked -- enroll --out template.json capture-0 capture-1 capture-2
cargo run --locked -- synthetic --identity 41 --impression 9 --out probe
cargo run --locked -- verify --template template.json probe
```

Named output paths must not already exist.
Verification exits 0 for a match, 1 for a non-match, and 2 for invalid input or a processing failure.

## Device workers

Match-on-chip devices keep the matcher and return opaque templates and match results.
The host-image path works with images, minutiae, and scores.
Their template kinds and policies are distinct, and each verification path rejects the other kind.
Unknown template versions are rejected.

The [libfprint worker](../crates/fpk-driver-libfprint) runs in a separate process and communicates over JSON Lines.
The root crate keeps its device dependencies and FFI outside the core.
The [hardware tooling](../docker) records a UPEK TouchStrip check with libfprint 1.94.10.
That result does not establish support for other devices.
The host-image path has no real sensor source.

Treat enrollment completion as a terminal event.
A device may finish without a final progress count, so waiting for completed stages to equal the total can hang.

## Data and security

Capture bundles hold a manifest and a binary PGM image.
Templates record their kind, engine, policy, and enrollment data.
Fingerprint captures and templates are sensitive biometric information.
The CLI creates persistent capture and template output only when the caller names an output path.
Synthetic captures contain no real person's biometrics.

See the [architecture](../ARCHITECTURE.md) and [security policy](../SECURITY.md) for protocol validation, resource limits, and future boundaries.

## Development

Run formatting, strict Clippy, tests, documentation, and the acceptance demo:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo doc --locked --no-deps
cargo run --locked -- demo
```

Set RUSTDOCFLAGS to `-D warnings` when checking documentation.
Run `mise x -- actionlint` to validate the workflows.
CI runs the native checks for source inputs and keeps documentation changes on the fast workflow gate.
If the repository uses an Actions allowlist, permit `dorny/paths-filter` and `jdx/mise-action`.
Require full commit references so Renovate can update their pins through verified pull requests.
