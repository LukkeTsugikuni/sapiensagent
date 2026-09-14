#!/usr/bin/env sh
set -eu

sapiens_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
user_home=${HOME:-}
rebuild=0
dry_run=0
for argument in "$@"; do
  case "$argument" in
    --rebuild) rebuild=1 ;;
    --dry-run) dry_run=1 ;;
    *) echo "Argumento desconhecido: $argument" >&2; exit 2 ;;
  esac
done
if [ -z "$user_home" ]; then
  echo 'HOME nao esta definido; nao foi possivel instalar o comando.' >&2
  exit 1
fi

sapiens_bin="$sapiens_root/target/release/sapiens-agent"
sapiens_bin_dir="$user_home/.local/bin"
if [ "$dry_run" -eq 1 ]; then
  echo "[dry-run] cargo build --release --manifest-path $sapiens_root/Cargo.toml"
else
  if [ "$rebuild" -eq 1 ] || [ ! -f "$sapiens_bin" ]; then
    backup="$sapiens_bin.previous"
    had_binary=0
    if [ -f "$sapiens_bin" ]; then cp -f "$sapiens_bin" "$backup"; had_binary=1; fi
    if cargo build --release --manifest-path "$sapiens_root/Cargo.toml" && [ -s "$sapiens_bin" ]; then
      [ "$had_binary" -eq 0 ] || rm -f "$backup"
    else
      if [ "$had_binary" -eq 1 ]; then cp -f "$backup" "$sapiens_bin"; fi
      echo 'Falha no build release; binário anterior restaurado.' >&2
      exit 1
    fi
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    echo "Release SHA-256: $(sha256sum "$sapiens_bin" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    echo "Release SHA-256: $(shasum -a 256 "$sapiens_bin" | awk '{print $1}')"
  fi
fi
if [ "$dry_run" -eq 1 ]; then
  echo "[dry-run] instalar links em $sapiens_bin_dir"
  exit 0
fi
mkdir -p "$sapiens_bin_dir"
ln -sfn "$sapiens_bin" "$sapiens_bin_dir/sapiens-agent"
ln -sfn "$sapiens_bin" "$sapiens_bin_dir/sapiens"

echo 'Sapiens Agent instalado.'
echo 'Garanta que ~/.local/bin está no PATH e use: sapiens-agent ou sapiens'
