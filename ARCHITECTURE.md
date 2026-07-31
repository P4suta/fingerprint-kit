# Architecture

M0 is intentionally one Rust package with a library and the `fingerprint-kit` binary. Crate
splitting and a public driver ABI are deferred until at least two real devices demonstrate a stable
boundary.

This is not a compatibility implementation or successor for fprint, libfprint, or fprintd. Real
devices, OS login, and production authentication are unsupported.

The data flow is:

```text
SyntheticSource / ReplaySource
        |
CanonicalImage + CaptureMetadata
        |
fprint-mindtct 0.1.0
        |
version-1 TemplateRecord JSON
        |
fprint-bozorth3 0.1.0, max score over 3 samples
        |
VerificationResult (experimental threshold 40)
```

`CaptureSource` is private, synchronous, and unstable. M0 implements only deterministic procedural
generation and filesystem replay. There is no async runtime, USB/SPI/HID access, hotplug, firmware,
or cancellation implementation.

The `protocol` module specifies bounded JSON Lines version 0 types and a session validator for a
future process boundary. Request IDs and operation IDs are separate. Encoded lines are limited to
2 MiB and decoded frames to 1 MiB. M0 does not spawn, supervise, or broker a driver process, and the
protocol is not a stable ABI.

Driver/process isolation, match-on-chip (MOC), encrypted or OS-backed secure storage, and PAM/D-Bus
adapters are separate future milestones. fprint/libfprint/fprintd compatibility, desktop
integration, PolicyKit, GNOME/KDE integration, and persistent credentials are outside M0.
FP3 import is also outside M0.

Capture images and templates are sensitive biometric information. They are saved only at an
explicitly named output, their payloads are never displayed or logged, and pixel/minutiae `Debug`
representations are redacted.

ISO/IEC 39794-4 and ISO/IEC 24745 inform terminology only; no conformance is claimed.
