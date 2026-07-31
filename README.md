# fingerprint-kit

[![CI](https://github.com/P4suta/fingerprint-kit/actions/workflows/ci.yml/badge.svg)](https://github.com/P4suta/fingerprint-kit/actions/workflows/ci.yml)
[![CodeQL](https://github.com/P4suta/fingerprint-kit/actions/workflows/codeql.yml/badge.svg)](https://github.com/P4suta/fingerprint-kit/actions/workflows/codeql.yml)
[![RustSec](https://github.com/P4suta/fingerprint-kit/actions/workflows/security-audit.yml/badge.svg)](https://github.com/P4suta/fingerprint-kit/actions/workflows/security-audit.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`fingerprint-kit` 0.1.0 is a hardware-free, experimental Rust 2024 vertical slice for fingerprint
capture replay, minutiae detection, three-sample enrollment, versioned template storage, and
verification. It uses `fprint-mindtct = 0.1.0` and `fprint-bozorth3 = 0.1.0` directly from
crates.io. No fingerprint hardware or biometric fixture is required.

This project is **not** an implementation, compatibility layer, or successor for fprint,
libfprint, or fprintd. It does not support real devices, OS login, or production authentication.
The policy `experimental-nbis-40-v1` is an uncalibrated engineering fixture; it makes no FMR, FNMR,
or PAD claim.

A secure template store and PAM/D-Bus adapters are separate future milestones. FP3 import is
outside M0, as are firmware, real USB/SPI/HID transport, hotplug, persistent credentials, and
desktop integration.

## Match-on-chip

M1 adds the second sensor archetype. A match-on-chip device keeps the matcher: the host never sees
a finger, receives an opaque template it cannot read, and hands that template back at verification
time for the sensor to compare against a live scan. The trust boundary is inverted, so the API is
too — [`MatchOnChipDevice`](src/moc.rs) has an enrollment that yields a blob and a verification that
yields a verdict, with no image, no minutiae, no score, and no threshold.

The two archetypes are kept unrepresentable in each other's terms. A template is stored under an
explicit `kind` discriminant (`host_image` or `match_on_chip`), the policies are disjoint
(`experimental-nbis-40-v1` against `device-match-v1`), and each fails closed when handed to the
other's path. Templates are schema version 2; a version-1 file no longer loads, because v1 had no
`kind` and guessing one would be the opposite of failing closed.

**Enrollment progress is not a countdown.** A driver need not report its final stage: libfprint's
`upekts` reports a stage only once the *following* poll asks for another presentation, so the poll
after the last presentation says "complete" and reports nothing — a 3-stage enrollment emits two
progress events. This was measured on a UPEK TouchStrip, not inferred. Treat the template's arrival
as the completion signal; waiting for `completed == total` hangs.

The `protocol` module carries the matching messages (`StartEnroll`, `StartVerify`, `EnrollProgress`,
`Enrolled`, `MatchResult`), and `SessionValidator` enforces that the operation a session opened is
the only one whose events it will accept.

### Drivers run in their own process

A device is reached through a **driver worker**: a separate program that speaks the protocol on
stdio. [`WorkerDevice`](src/worker.rs) spawns one and presents it as an ordinary
`MatchOnChipDevice`, so nothing above that module knows the device is in another process.

This is what lets the root crate stay `forbid(unsafe_code)`, permissively licensed, and free of
any device dependency while still driving real hardware. `crates/fpk-driver-libfprint` is the
first worker: it delegates to libfprint through the published `fprint-backend-libfprint` shim, and
because it is a separate *process*, none of that — the FFI, the `unsafe`, the LGPL — crosses into
the core. Replacing it with a native Rust driver later changes nothing above the pipe.

```console
cargo build --workspace
fingerprint-kit enroll-device --out template.json --driver ./target/debug/fpk-driver-libfprint
fingerprint-kit verify-device --template template.json --driver ./target/debug/fpk-driver-libfprint
```

`verify-device` exits 0 for a match and 1 for a non-match, like `verify`.

### Hardware status

The match-on-chip path has been run **on one real sensor**: a UPEK TouchStrip (`0483:2016`,
libfprint driver `upekts`), enrolled and verified end to end through the CLI, the worker protocol,
and libfprint 1.94.10, with a non-matching finger correctly rejected. `docker/` holds the
container that did it; `docker/bringup.sh` is the script.

Nothing else is hardware-verified. The host-image path has still never seen a sensor — there is no
`CaptureSource` that reads from one — and one device does not generalize to a second driver, let
alone to the other archetype. Note also that libfprint before **1.94.9** cannot verify on this
sensor at all (`upekts` verify is broken; fixed upstream by `cdc22b45`), which is why the container
builds libfprint from source rather than using a distribution package.

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
