# fingerprint-kit

[![CI](https://github.com/P4suta/fingerprint-kit/actions/workflows/ci.yml/badge.svg)](https://github.com/P4suta/fingerprint-kit/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`fingerprint-kit` 0.1.0 is a hardware-free, experimental Rust 2024 vertical slice for fingerprint
capture replay, minutiae detection, three-sample enrollment, versioned template storage, and
verification. It uses `fprint-mindtct = 0.1.0` and `fprint-bozorth3 = 0.1.0` directly from
crates.io. No fingerprint hardware or biometric fixture is required.

This project is **not** an implementation, compatibility layer, or successor for fprint,
libfprint, or fprintd. It does not support real devices, OS login, or production authentication.
The policy `experimental-nbis-40-v1` is an uncalibrated engineering fixture; it makes no FMR, FNMR,
or PAD claim.

A driver/process boundary, match-on-chip (MOC), a secure template store, and PAM/D-Bus adapters are
separate future milestones. FP3 import is outside M0, as are firmware, real USB/SPI/HID transport,
hotplug, persistent credentials, and desktop integration.

## Quick start

Rust 1.97.1 is required and selected automatically by `rust-toolchain.toml`. Build from the
repository with the locked dependency set:

```console
cargo build --locked
```

The complete acceptance demonstration stores all artifacts in a temporary directory and removes
them on completion:

```console
cargo run --locked -- demo
```

A manual round trip:

```console
cargo run --locked -- synthetic --identity 41 --impression 0 --out capture-0
cargo run --locked -- synthetic --identity 41 --impression 1 --out capture-1
cargo run --locked -- synthetic --identity 41 --impression 2 --out capture-2
cargo run --locked -- inspect capture-0
cargo run --locked -- enroll --out template.json capture-0 capture-1 capture-2
cargo run --locked -- synthetic --identity 41 --impression 9 --out probe
cargo run --locked -- verify --template template.json probe
```

`verify` exits 0 for a match, 1 for a normal non-match, and 2 for invalid input or processing
failure. Output capture directories and template files must not already exist.

## Data formats and policy

A capture bundle is a directory containing exactly the fixed inputs `capture.json` (manifest
version 1) and `image.pgm` (binary P5, maxval 255). The canonical pixels are packed Gray8,
top-left origin, X rightward, Y downward, with dark ridges. Images are bounded to 16 MiB.

A template is explicit version-1 JSON containing only engine/version, capture profile, policy, and
three quality-bearing minutiae samples. It does not contain a user name, finger label, source image,
or capture time. Template input is bounded to 1 MiB and fails closed on unknown schema, engine,
policy, profile, sample count, minutia count, and quality violations.

M0 fixes enrollment at three samples, requires at least
`MIN_COMPUTABLE_BOZORTH_MINUTIAE` minutiae and mean quality 25/100 per capture, then takes the
maximum BOZORTH3 score over the three samples. A score of 40 or greater is a match.

ISO/IEC 39794-4 and ISO/IEC 24745 are referenced only as design vocabulary. This project does not
claim conformance to either standard.

## Security and scope

Fingerprint captures and templates are sensitive biometric information. `fingerprint-kit` does
not persist them unless the caller explicitly names a capture or template output. It never displays
or logs their payloads, and pixel/minutiae values are redacted from `Debug`. Synthetic data is
procedural and carries no real person's biometrics.

See [ARCHITECTURE.md](ARCHITECTURE.md) and [SECURITY.md](SECURITY.md) for boundaries, limits, and
future milestones.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
