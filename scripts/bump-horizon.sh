#!/usr/bin/env bash
# Move the Horizon revision Zenith is built against, run the tests, and say what broke.
set -euo pipefail

horizon_url=https://github.com/horizon-rust/horizon
horizon_paths=(crates/horizon-protocol crates/horizon-core crates/horizon-world)

run_tests=1
want=
for arg in "$@"; do
  case $arg in
    --no-test) run_tests=0 ;;
    -h | --help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "unknown option: $arg" >&2
      exit 2
      ;;
    *)
      if [ -n "$want" ]; then
        echo "only one revision can be given" >&2
        exit 2
      fi
      want=$arg
      ;;
  esac
done

cd "$(dirname "$0")/.."

old=$(sed -n 's|.*horizon-rust/horizon", rev = "\([0-9a-f]\{40\}\)".*|\1|p' Cargo.toml | head -n 1)
if [ "${#old}" -ne 40 ]; then
  echo "Cargo.toml has no horizon-protocol line with a 40-digit rev" >&2
  exit 2
fi

if [[ $want =~ ^[0-9a-f]{40}$ ]]; then
  new=$want
else
  if [ -z "$want" ]; then
    found=$(git ls-remote "$horizon_url" HEAD)
  else
    found=$(git ls-remote "$horizon_url" "refs/heads/$want" "refs/tags/$want^{}" "refs/tags/$want")
  fi
  new=$(printf '%s\n' "$found" | sed -n '1s/[[:space:]].*//p')
  if [[ ! $new =~ ^[0-9a-f]{40}$ ]]; then
    echo "cannot find ${want:-the head of the default branch} in $horizon_url" >&2
    exit 2
  fi
fi

echo "Horizon: ${old:0:12} -> ${new:0:12}"
if [ "$old" = "$new" ]; then
  echo "(this is the revision already pinned)"
fi

rewrite() {
  local tmp
  tmp=$(mktemp)
  sed "$2" "$1" >"$tmp"
  cat "$tmp" >"$1"
  rm -f "$tmp"
}
rewrite Cargo.toml "s|\\(horizon-rust/horizon\", rev = \"\\)[0-9a-f]\\{40\\}|\\1$new|"

cargo fetch

report=${BUMP_REPORT:-}
[ -z "$report" ] || : >"$report"
say() {
  printf '%s\n' "$*"
  if [ -n "$report" ]; then printf '%s\n' "$*" >>"$report"; fi
}

say "Horizon ${old:0:12} -> ${new:0:12}: $horizon_url/compare/${old:0:12}...${new:0:12}"
say ""

if [ "$run_tests" -eq 0 ]; then
  say "Not tested (--no-test)."
  exit 0
fi

log=$(mktemp)
trap 'rm -f "$log"' EXIT
if cargo test --workspace 2>&1 | tee "$log"; then
  say "Build and tests pass against ${new:0:12}."
  say ""
  say "To keep it: commit Cargo.toml and Cargo.lock."
  exit 0
fi

say "Build or tests fail against ${new:0:12}."
say ""
errors=$(grep -E -A8 -m3 '^error' "$log" | head -c 3000 || true)
failed=$(grep -E '^test .* FAILED$|^---- .* ----$' "$log" | sed -n '1,20p' || true)
{
  if [ -n "$errors" ]; then
    echo "Compiler errors (first three):"
    echo
    echo '```'
    printf '%s\n' "$errors" | sed 's/```/'"'''"'/g'
    echo '```'
  fi
  if [ -n "$failed" ]; then
    echo
    echo "Failed tests:"
    echo
    echo '```'
    printf '%s\n' "$failed" | sed 's/```/'"'''"'/g'
    echo '```'
  fi
} | while IFS= read -r line; do say "$line"; done
say ""
say "The files keep the new revision. To go back: git checkout -- Cargo.toml Cargo.lock"
exit 1
