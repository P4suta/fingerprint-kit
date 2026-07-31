#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Enroll and then verify a real finger on a match-on-chip sensor, through the whole stack:
# CLI -> WorkerDevice -> JSON Lines -> driver worker -> libfprint -> USB.
#
# Run it inside the bring-up container:
#   docker compose -f docker/docker-compose.hw.yml run --rm -T hw bash docker/bringup.sh
#
# Enrollment and verification must happen in ONE container run. The template is written to
# /run/bringup, which lives in the container's own filesystem and dies with `--rm`, so a real
# finger's template never reaches the host or the working tree. Do not "fix" this by writing to
# /work — that is the repository. See SECURITY.md.
set -euo pipefail

KIT=./target/debug/fingerprint-kit
DRIVER=./target/debug/fpk-driver-libfprint
TEMPLATE=/run/bringup/template.json

for binary in "$KIT" "$DRIVER"; do
    [ -x "$binary" ] || { echo "missing $binary — run: cargo build --workspace" >&2; exit 1; }
done
mkdir -p /run/bringup
rm -f "$TEMPLATE"

echo "=== ENROLL ==="
"$KIT" enroll-device --out "$TEMPLATE" --driver "$DRIVER"

echo
echo "=== VERIFY: present the SAME finger ==="
if "$KIT" verify-device --template "$TEMPLATE" --driver "$DRIVER"; then
    echo "  -> matched, as expected"
else
    status=$?
    echo "  -> UNEXPECTED: the enrolled finger did not match (exit $status)" >&2
    exit 1
fi

echo
echo "=== VERIFY: present a DIFFERENT finger ==="
# The check that separates "matching works" from "always says yes".
set +e
"$KIT" verify-device --template "$TEMPLATE" --driver "$DRIVER"
status=$?
set -e
case "$status" in
    1) echo "  -> correctly rejected" ;;
    0) echo "  -> UNEXPECTED: a different finger matched" >&2; exit 1 ;;
    *) echo "  -> error while verifying (exit $status)" >&2; exit 1 ;;
esac

echo
echo "=== the whole stack works on real hardware ==="
