#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RPM_TOPDIR="${RPM_TOPDIR:-$ROOT_DIR/target/rpmbuild}"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
PROFILE_BIN="$ROOT_DIR/target/$TARGET/release/diskmon-mail-v2"

if command -v cross >/dev/null 2>&1; then
  cross build --release --target "$TARGET"
else
  cargo build --release --target "$TARGET"
fi

mkdir -p "$RPM_TOPDIR"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
install -m 0755 "$PROFILE_BIN" "$RPM_TOPDIR/SOURCES/diskmon-mail-v2"
install -m 0644 "$ROOT_DIR/src/linux/config.example.yaml" "$RPM_TOPDIR/SOURCES/config.example.yaml"
install -m 0644 "$ROOT_DIR/packaging/systemd/diskmon-v2.service" "$RPM_TOPDIR/SOURCES/diskmon-v2.service"
install -m 0644 "$ROOT_DIR/packaging/systemd/diskmon-v2.timer" "$RPM_TOPDIR/SOURCES/diskmon-v2.timer"
install -m 0644 "$ROOT_DIR/packaging/systemd/diskmon-v2-force.service" "$RPM_TOPDIR/SOURCES/diskmon-v2-force.service"
install -m 0644 "$ROOT_DIR/packaging/systemd/diskmon-v2-force.timer" "$RPM_TOPDIR/SOURCES/diskmon-v2-force.timer"
install -m 0644 "$ROOT_DIR/packaging/rpm/diskmon-mail-v2.spec" "$RPM_TOPDIR/SPECS/diskmon-mail-v2.spec"

rpmbuild --define "_topdir $RPM_TOPDIR" -bb "$RPM_TOPDIR/SPECS/diskmon-mail-v2.spec"
