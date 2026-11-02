#!/usr/bin/env bash
# Installs the newest jmap-sdk release into ~/.local/bin:
#   curl -fsSL https://raw.githubusercontent.com/imabee-glacier/jmap-sdk/main/install/jmap-sdk.sh | bash
set -euo pipefail

REPO="imabee-glacier/jmap-sdk"
COMPONENT="jmap-sdk"
BIN_DIR="${HOME}/.local/bin"
tmp=""

die() {
  printf 'install: %s\n' "$*" >&2
  exit 1
}

detect_triple() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}/${arch}" in
    Linux/x86_64) echo "x86_64-unknown-linux-gnu" ;;
    Linux/aarch64 | Linux/arm64) echo "aarch64-unknown-linux-gnu" ;;
    Darwin/arm64) echo "aarch64-apple-darwin" ;;
    Darwin/x86_64) echo "x86_64-apple-darwin" ;;
    *) die "unsupported platform: ${os} ${arch}" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "need sha256sum or shasum to verify the download"
  fi
}

main() {
  command -v curl >/dev/null 2>&1 || die "curl is required"
  command -v tar >/dev/null 2>&1 || die "tar is required"

  local triple tag version archive base expected actual
  triple="$(detect_triple)"

  # Newest published ${COMPONENT}-vX.Y.Z release; other components' tags are ignored.
  version="$(
    curl -fsSL "https://api.github.com/repos/${REPO}/releases?per_page=100" |
      grep -o "\"tag_name\": *\"${COMPONENT}-v[0-9]*\.[0-9]*\.[0-9]*\"" |
      sed -e "s/.*-v\([0-9.]*\)\"\$/\1/" |
      sort -t. -k1,1n -k2,2n -k3,3n |
      tail -n 1
  )" || true
  [ -n "${version}" ] || die "no ${COMPONENT} release found in ${REPO}"
  tag="${COMPONENT}-v${version}"

  archive="${COMPONENT}-${version}-${triple}.tar.gz"
  base="https://github.com/${REPO}/releases/download/${tag}"

  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp:-}"' EXIT

  printf 'Installing %s %s (%s)\n' "${COMPONENT}" "${version}" "${triple}"
  curl -fsSL -o "${tmp}/${archive}" "${base}/${archive}" ||
    die "no ${triple} build in release ${tag}"
  curl -fsSL -o "${tmp}/${archive}.sha256" "${base}/${archive}.sha256" ||
    die "missing checksum for ${archive}"

  expected="$(cut -d' ' -f1 "${tmp}/${archive}.sha256")"
  actual="$(sha256_of "${tmp}/${archive}")"
  [ -n "${expected}" ] && [ "${expected}" = "${actual}" ] ||
    die "SHA256 mismatch for ${archive}"

  mkdir -p "${tmp}/unpack" "${BIN_DIR}"
  tar -xzf "${tmp}/${archive}" -C "${tmp}/unpack"
  [ -f "${tmp}/unpack/${COMPONENT}" ] || die "archive does not contain ${COMPONENT}"
  install -m 0755 "${tmp}/unpack/${COMPONENT}" "${BIN_DIR}/${COMPONENT}.new"
  mv -f "${BIN_DIR}/${COMPONENT}.new" "${BIN_DIR}/${COMPONENT}"

  printf 'Installed %s\n' "${BIN_DIR}/${COMPONENT}"
  case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *) printf 'Add %s to your PATH to run it.\n' "${BIN_DIR}" ;;
  esac
}

main "$@"
