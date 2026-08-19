#!/bin/sh
#
# Astra CLI one-line installer (Linux / macOS).
#
#   curl -fsSL https://astracode.topodrive.top/install/install.sh | sh
#
# Override the target directory with:
#   ASTRA_INSTALL_DIR=/opt/astra ./install.sh
set -eu

INSTALL_DIR="${ASTRA_INSTALL_DIR:-$HOME/.local/bin}"
REPO="${ASTRA_REPO:-kevenhu001-cyber/astra-code-v3}"

os="$(uname -s)"
arch="$(uname -m)"

case "$arch" in
  x86_64|amd64) target_arch="x86_64" ;;
  aarch64|arm64) target_arch="aarch64" ;;
  *)
    echo "error: unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

case "$os" in
  Linux) target_os="unknown-linux-gnu" ;;
  Darwin) target_os="apple-darwin" ;;
  *)
    echo "error: unsupported OS: $os (use the Windows installer instead)" >&2
    exit 1
    ;;
esac

# Fetch latest release version from GitHub API
echo "Fetching latest release info..."
LATEST_URL="https://api.github.com/repos/$REPO/releases/latest"
TAG=$(curl -fsSL "$LATEST_URL" | grep "\"tag_name\"" | sed -E "s/.*\"tag_name\": *\"([^\"]+)\".*/\1/")
if [ -z "$TAG" ]; then
  echo "error: failed to fetch latest release version" >&2
  exit 1
fi
echo "Latest release: $TAG"

asset="astra-${TAG}-${target_arch}-${target_os}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/$TAG"

tmpdir="$(mktemp -d)"
trap "rm -rf \"\$tmpdir\"" EXIT HUP INT TERM

echo "Downloading $asset ..."
curl -fsSL "$BASE_URL/$asset" -o "$tmpdir/$asset"
curl -fsSL "$BASE_URL/$asset.sha256" -o "$tmpdir/$asset.sha256"

# Verify the download against the published SHA-256 checksum.
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmpdir" && sha256sum -c "$asset.sha256") || {
    echo "error: checksum verification failed for $asset" >&2
    exit 1
  }
elif command -v shasum >/dev/null 2>&1; then
  (cd "$tmpdir" && shasum -a 256 -c "$asset.sha256") || {
    echo "error: checksum verification failed for $asset" >&2
    exit 1
  }
else
  echo "warning: no sha256 tool found; skipping checksum verification" >&2
fi

tar -xzf "$tmpdir/$asset" -C "$tmpdir"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmpdir/astra" "$INSTALL_DIR/astra"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
      [ -f "$rc" ] || continue
      grep -Fq "$INSTALL_DIR" "$rc" 2>/dev/null && continue
      printf "\n# added by astra installer\nexport PATH=\"%s:\$PATH\"\n" "$INSTALL_DIR" >> "$rc"
    done
    echo "Added $INSTALL_DIR to your shell config (new terminals will pick it up)."
    ;;
esac

# Make `astra` available in the current shell too.
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    export PATH="$INSTALL_DIR:$PATH"
    echo "Updated PATH for this shell."
    ;;
esac

"$INSTALL_DIR/astra" version
echo "Astra installed: $INSTALL_DIR/astra"
