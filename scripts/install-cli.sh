#!/usr/bin/env sh
set -eu

repo="socai-io/socai"
asset="socai-cli-macos-universal.tar.gz"
checksum="${asset}.sha256"
default_base_url="https://socai-download.oss-cn-beijing.aliyuncs.com/releases/latest"
github_base_url="https://github.com/${repo}/releases/latest/download"
base_url="${SOCAI_DOWNLOAD_BASE_URL:-$default_base_url}"
install_dir="${SOCAI_INSTALL_DIR:-$HOME/.socai/bin}"

case "$(uname -s 2>/dev/null || echo unknown)" in
  Darwin) ;;
  *)
    echo "socai CLI release binary install currently supports macOS only." >&2
    echo "For other platforms, use the source/Cargo fallback in README.md." >&2
    exit 1
    ;;
esac

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required to install socai" >&2
  exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
  echo "tar is required to install socai" >&2
  exit 1
fi
if ! command -v shasum >/dev/null 2>&1; then
  echo "shasum is required to verify socai" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

archive="$tmp_dir/$asset"
checksum_file="$tmp_dir/$checksum"
unpack_dir="$tmp_dir/unpack"

downloaded=false
attempted_base_url=""
for candidate_base_url in "$base_url" "$github_base_url"; do
  if [ "$candidate_base_url" = "$attempted_base_url" ]; then
    continue
  fi
  attempted_base_url="$candidate_base_url"

  printf 'downloading socai CLI from %s\n' "$candidate_base_url"
  rm -f "$archive" "$checksum_file"
  if curl -fL "$candidate_base_url/$asset" -o "$archive" &&
    curl -fL "$candidate_base_url/$checksum" -o "$checksum_file" &&
    (cd "$tmp_dir" && shasum -a 256 -c "$checksum"); then
    base_url="$candidate_base_url"
    downloaded=true
    break
  fi
  printf 'download or checksum verification failed from %s\n' "$candidate_base_url" >&2
done

if [ "$downloaded" != "true" ]; then
  echo "failed to download a verified socai CLI release" >&2
  exit 1
fi

mkdir -p "$unpack_dir" "$install_dir"
tar -xzf "$archive" -C "$unpack_dir"

if [ ! -f "$unpack_dir/socai" ]; then
  echo "release archive did not contain ./socai" >&2
  exit 1
fi
if [ ! -f "$unpack_dir/socai-asr" ]; then
  echo "release archive did not contain ./socai-asr" >&2
  exit 1
fi

install -m 0755 "$unpack_dir/socai" "$install_dir/socai"
install -m 0755 "$unpack_dir/socai-asr" "$install_dir/socai-asr"

printf 'installed socai to %s\n' "$install_dir/socai"
"$install_dir/socai" --version

case ":$PATH:" in
  *":$install_dir:"*)
    printf '%s is already on PATH in this shell\n' "$install_dir"
    ;;
  *)
    printf '\nAdd socai to PATH for future shells:\n'
    printf '  export PATH="%s:$PATH"\n' "$install_dir"

    shell_rc=""
    shell_name="$(basename "${SHELL:-}" 2>/dev/null || true)"
    case "$shell_name" in
      zsh) shell_rc="$HOME/.zshrc" ;;
      bash) shell_rc="$HOME/.bashrc" ;;
    esac

    if [ -n "$shell_rc" ]; then
      mkdir -p "$(dirname "$shell_rc")"
      touch "$shell_rc"
      if grep -F "$install_dir" "$shell_rc" >/dev/null 2>&1; then
        printf '%s already mentions %s\n' "$shell_rc" "$install_dir"
      else
        {
          printf '\n# socai CLI\n'
          printf 'export PATH="%s:$PATH"\n' "$install_dir"
        } >> "$shell_rc"
        printf 'updated %s\n' "$shell_rc"
      fi
    else
      printf 'Could not infer shell rc file; add the PATH line above manually.\n'
    fi
    ;;
esac
