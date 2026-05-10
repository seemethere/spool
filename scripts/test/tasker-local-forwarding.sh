#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)
TMPDIR=${TMPDIR:-/tmp}
TEST_ROOT=$(mktemp -d "$TMPDIR/tasker-local-test.XXXXXX")
TEST_ROOT=$(CDPATH= cd -- "$TEST_ROOT" && pwd -P)
trap 'rm -rf "$TEST_ROOT"' EXIT INT TERM

mkdir -p "$TEST_ROOT/bin" "$TEST_ROOT/target/debug" "$TEST_ROOT/.tasker"
cp "$ROOT/bin/tasker-local" "$TEST_ROOT/bin/tasker-local"
printf '[database]\npath = "%s"\n' "$TEST_ROOT/tasker.db" > "$TEST_ROOT/.tasker/config.toml"

cat > "$TEST_ROOT/target/debug/tasker" <<'FAKE'
#!/usr/bin/env sh
printf '%s\n' "$@" > "$TASKER_LOCAL_CAPTURE"
FAKE
chmod +x "$TEST_ROOT/target/debug/tasker"

CAPTURE="$TEST_ROOT/args.txt"
TASKER_LOCAL_CAPTURE="$CAPTURE" "$TEST_ROOT/bin/tasker-local" queue list --format json 'space value'

EXPECTED="$TEST_ROOT/expected.txt"
cat > "$EXPECTED" <<EOF_EXPECTED
--config
$TEST_ROOT/.tasker/config.toml
queue
list
--format
json
space value
EOF_EXPECTED

diff -u "$EXPECTED" "$CAPTURE"

rm "$TEST_ROOT/target/debug/tasker"
if "$TEST_ROOT/bin/tasker-local" status >"$TEST_ROOT/missing.out" 2>"$TEST_ROOT/missing.err"; then
  echo "expected missing binary failure" >&2
  exit 1
fi
if ! grep -q "cargo build -p tasker-cli" "$TEST_ROOT/missing.err"; then
  echo "missing binary error did not suggest cargo build" >&2
  cat "$TEST_ROOT/missing.err" >&2
  exit 1
fi
