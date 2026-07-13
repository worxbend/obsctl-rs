#!/bin/sh
# Installs the obsctl-rs CLI from GitHub Releases.
#
#   curl --proto '=https' --tlsv1.2 -sSf https://github.com/worxbend/obsctl-rs/releases/latest/download/install.sh | sh
#
# Env vars:
#   OBSCTL_VERSION      release tag to install, e.g. "v0.2.0" (default: latest)
#   OBSCTL_INSTALL_DIR  install directory (default: "$HOME/.local/bin")
set -eu

REPO="worxbend/obsctl-rs"
BIN_NAME="obsctl"
INSTALL_DIR="${OBSCTL_INSTALL_DIR:-$HOME/.local/bin}"

err() {
  echo "error: $1" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required but was not found"
}

os=$(uname -s)
arch=$(uname -m)

case "$os" in
  Linux) ;;
  *) err "unsupported OS: $os (obsctl-rs currently ships Linux binaries only)" ;;
esac

case "$arch" in
  x86_64 | amd64) asset_arch="amd64" ;;
  *) err "unsupported architecture: $arch (only x86_64 is currently supported)" ;;
esac

need_cmd curl
need_cmd tar
need_cmd sha256sum
need_cmd mktemp

version="${OBSCTL_VERSION:-}"
if [ -z "$version" ]; then
  latest_url=$(curl --proto '=https' --tlsv1.2 -fsSL -o /dev/null -w '%{url_effective}' \
    "https://github.com/${REPO}/releases/latest")
  version="${latest_url##*/}"
fi
[ -n "$version" ] || err "could not determine the latest release version"

asset="obsctl-${version}-linux-${asset_arch}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/${version}"

tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT

echo "Downloading obsctl-rs ${version} (linux-${asset_arch})..."
curl --proto '=https' --tlsv1.2 -fsSL -o "$tmp_dir/$asset" "$base_url/$asset" \
  || err "failed to download $asset from $base_url"
curl --proto '=https' --tlsv1.2 -fsSL -o "$tmp_dir/$asset.sha256" "$base_url/$asset.sha256" \
  || err "failed to download $asset.sha256 from $base_url"

(cd "$tmp_dir" && sha256sum -c "$asset.sha256") || err "checksum verification failed"

tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"

mkdir -p "$INSTALL_DIR"
cp "$tmp_dir/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
chmod +x "$INSTALL_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $INSTALL_DIR/$BIN_NAME"

add_path_line() {
  rc_file="$1"
  marker="# added by obsctl-rs installer"

  if [ -f "$rc_file" ] && grep -qF "$marker" "$rc_file" 2>/dev/null; then
    return 0
  fi
  printf '\nexport PATH="%s:$PATH" %s\n' "$INSTALL_DIR" "$marker" >> "$rc_file"
  echo "Added $INSTALL_DIR to PATH in $rc_file"
}

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    add_path_line "$HOME/.bashrc"
    add_path_line "$HOME/.zshrc"
    echo "Restart your shell (or run 'source ~/.bashrc' / 'source ~/.zshrc') to update PATH."
    ;;
esac

echo "Run '$BIN_NAME --help' to get started."
