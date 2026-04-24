set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

cargo_manifest := "Cargo.toml"
windows_bins := '["wine2linux"]'
host_target := `rustc -vV | sed -n 's/^host: //p'`
host_arch := `rustc -vV | sed -n 's/^host: //p' | sed 's/\([^-]*\).*/\1/'`
windows_target := host_arch + "-pc-windows-gnu"
bin_targets := `cargo metadata --format-version 1 --no-deps --manifest-path Cargo.toml | jq -r '.packages[] | select(.manifest_path | test("/crates/")) | .name as $pkg | .targets[] | select(.kind == ["bin"]) | "\($pkg):\(.name)"' | sort -u`

[default]
default:
  @just --list

list:
  @printf '%s\n' '{{bin_targets}}'

clean:
  cargo clean --manifest-path {{cargo_manifest}}

[private]
_build target profile="debug":
  #!/usr/bin/env bash
  set -x -eu -o pipefail
  case "{{target}}" in
    *:*) ;;
    *)
      echo "target must be <crate-name>:<binary-name>" >&2
      exit 1
      ;;
  esac
  case "{{profile}}" in
    debug|release) ;;
    *)
      echo "profile must be 'debug' or 'release'" >&2
      exit 1
      ;;
  esac
  target_value="{{target}}"
  package="${target_value%%:*}"
  bin="${target_value#*:}"
  profile_flag=()
  if [[ "{{profile}}" == "release" ]]; then
    profile_flag+=(--release)
  fi
  if jq -e --arg bin "${bin}" 'index($bin) != null' >/dev/null <<< '{{windows_bins}}'; then
    echo "==> building {{target}} for {{windows_target}} [{{profile}}]"
    cargo build --manifest-path {{cargo_manifest}} --package "${package}" --bin "${bin}" --target {{windows_target}} "${profile_flag[@]}"
  else
    echo "==> building {{target}} for host ({{host_target}}) [{{profile}}]"
    cargo build --manifest-path {{cargo_manifest}} --package "${package}" --bin "${bin}" "${profile_flag[@]}"
  fi

[arg("release", long, value="release")]
build target release="debug":
  just _build "{{target}}" "{{release}}"

[arg("release", long, value="release")]
build-all release="debug":
  #!/usr/bin/env bash
  set -eu -o pipefail
  while IFS= read -r target; do
    just _build "${target}" "{{release}}"
  done <<< '{{bin_targets}}'
