#!/usr/bin/env bash
# Build release binaries and package them for the current platform.
set -euo pipefail

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
TARGET_DIR="$ROOT_DIR/target/release"
DIST_DIR="$ROOT_DIR/dist"
STAGE_DIR="$DIST_DIR/noren-tools-staging"
PKG_NAME="noren-tools"

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

detect_version() {
  python - <<'PY'
import json, subprocess, sys
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"]))
print(metadata["packages"][0]["version"])
PY
}

prepare_binaries() {
  cargo build --release --bins

  rm -rf "$STAGE_DIR"
  mkdir -p "$STAGE_DIR/usr/local/bin"

  install -m 0755 "$TARGET_DIR/dbgen" "$STAGE_DIR/usr/local/bin/dbgen"
  install -m 0755 "$TARGET_DIR/rdbinspect" "$STAGE_DIR/usr/local/bin/rdbinspect"
}

build_tar() {
  local version="$1"
  local archive="$DIST_DIR/${PKG_NAME}-${version}.tar.gz"
  rm -f "$archive"
  tar -czf "$archive" -C "$STAGE_DIR" .
  echo "Created tarball: $archive"
}

build_deb() {
  local version="$1"
  local arch
  if command_exists dpkg; then
    arch=$(dpkg --print-architecture)
  else
    arch=$(uname -m)
  fi

  local deb_dir="$DIST_DIR/${PKG_NAME}_deb"
  local control_dir="$deb_dir/DEBIAN"

  rm -rf "$deb_dir"
  mkdir -p "$control_dir"
  cp -r "$STAGE_DIR"/usr "$deb_dir/"

  cat >"$control_dir/control" <<EOF
Package: $PKG_NAME
Version: $version
Section: utils
Priority: optional
Architecture: $arch
Maintainer: Noren Team
Description: Tooling binaries for Noren (dbgen and rdbinspect)
EOF

  local output="$DIST_DIR/${PKG_NAME}_${version}_${arch}.deb"
  fakeroot dpkg-deb --build "$deb_dir" "$output"
  echo "Created Debian package: $output"
}

build_rpm() {
  local version="$1"
  local sanitized_version=${version//-/.}
  local arch
  arch=$(uname -m)

  local rpm_root="$DIST_DIR/rpm"
  local spec_file="$rpm_root/SPECS/${PKG_NAME}.spec"

  rm -rf "$rpm_root"
  mkdir -p "$rpm_root/BUILD" "$rpm_root/RPMS" "$rpm_root/SOURCES" "$rpm_root/SPECS" "$rpm_root/SRPMS"

  install -m 0755 "$STAGE_DIR/usr/local/bin/dbgen" "$rpm_root/SOURCES/dbgen"
  install -m 0755 "$STAGE_DIR/usr/local/bin/rdbinspect" "$rpm_root/SOURCES/rdbinspect"

  cat >"$spec_file" <<EOF
Name:           $PKG_NAME
Version:        $sanitized_version
Release:        1%{?dist}
Summary:        Noren tooling binaries (dbgen and rdbinspect)

License:        Proprietary
BuildArch:      $arch
Source0:        dbgen
Source1:        rdbinspect

%description
Binaries for generating and inspecting Noren databases.

%prep
%build

%install
mkdir -p %{buildroot}/usr/local/bin
install -m 0755 %{SOURCE0} %{buildroot}/usr/local/bin/dbgen
install -m 0755 %{SOURCE1} %{buildroot}/usr/local/bin/rdbinspect

%files
/usr/local/bin/dbgen
/usr/local/bin/rdbinspect

%changelog
* $(date '+%a %b %d %Y') Noren Team - ${sanitized_version}-1
- Automated package build
EOF

  rpmbuild -bb "$spec_file" \
    --define "_topdir $rpm_root" \
    --define "_rpmdir $DIST_DIR" \
    --buildroot "$STAGE_DIR" \
    --target "$arch"

  echo "Created RPM package under $DIST_DIR"
}

detect_format() {
  if [[ "${PACKAGE_FORMAT:-auto}" != "auto" ]]; then
    echo "$PACKAGE_FORMAT"
    return
  fi

  if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    source /etc/os-release
    if [[ "${ID_LIKE:-}" == *"debian"* || "${ID:-}" == *"debian"* ]]; then
      echo "deb"
      return
    fi
    if [[ "${ID_LIKE:-}" == *"rhel"* || "${ID_LIKE:-}" == *"fedora"* || "${ID:-}" == *"rhel"* ]]; then
      echo "rpm"
      return
    fi
  fi

  echo "tar"
}

main() {
  local format
  format=$(detect_format)
  local version
  version=$(detect_version)

  prepare_binaries

  case "$format" in
    deb)
      command_exists dpkg-deb || { echo "dpkg-deb not available; cannot build .deb" >&2; exit 1; }
      command_exists fakeroot || { echo "fakeroot not available; install it to build .deb packages" >&2; exit 1; }
      build_deb "$version"
      ;;
    rpm)
      command_exists rpmbuild || { echo "rpmbuild not available; cannot build .rpm" >&2; exit 1; }
      build_rpm "$version"
      ;;
    tar)
      build_tar "$version"
      ;;
    *)
      echo "Unknown package format: $format" >&2
      exit 1
      ;;
  esac
}

main "$@"
