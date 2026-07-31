# Security

## Reporting a vulnerability

Do not report suspected vulnerabilities in a public issue. Use
[GitHub private vulnerability reporting](https://github.com/P4suta/fingerprint-kit/security/advisories/new)
so the report and any sensitive reproduction details remain private.

## Security model

M0 is an experimental, hardware-free pipeline. It is not suitable for production authentication,
OS login, authorization, or biometric risk decisions. The fixed score threshold has not been
calibrated for FMR/FNMR and there is no presentation-attack detection.

This project is not compatible with, or a successor to, fprint, libfprint, or fprintd. Real devices
are unsupported. ISO/IEC 39794-4 and ISO/IEC 24745 provide design vocabulary only; no conformance is
claimed.

Capture images and minutiae templates are sensitive biometric data:

- Persistent output is created only by commands whose caller explicitly supplies `--out`; `demo`
  uses an automatically removed temporary directory.
- Output paths are created new and are never implicitly overwritten.
- Raw pixels and minutiae payloads are redacted from `Debug` and are not printed by the CLI.
- Capture files are limited to 16 MiB, manifests to 64 KiB, templates to 1 MiB, protocol lines to
  2 MiB, and decoded protocol frames to 1 MiB.
- Version, engine, policy, capture profile, geometry, count, and quality mismatches fail closed.

The JSON template is not encrypted. Its explicit versioning and limits are parsing controls, not a
secure store. Protect explicitly saved artifacts with appropriate filesystem permissions and
delete them according to the applicable biometric-data policy.

Future real-device work must independently address process isolation, device trust, firmware,
secret storage, cancellation, hotplug, transport denial of service, secure deletion, and
platform-specific authentication integration.

The driver/process boundary, match-on-chip (MOC), a secure store, and PAM/D-Bus adapters are
independent future milestones. FP3 import is outside M0.
