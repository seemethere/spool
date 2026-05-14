#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)
TMPDIR=${TMPDIR:-/tmp}
TEST_ROOT=$(mktemp -d "$TMPDIR/spool-local-test.XXXXXX")
TEST_ROOT=$(CDPATH= cd -- "$TEST_ROOT" && pwd -P)
trap 'rm -rf "$TEST_ROOT"' EXIT INT TERM

mkdir -p "$TEST_ROOT/bin" "$TEST_ROOT/.spool" "$TEST_ROOT/fake-bin"
cp "$ROOT/bin/spool-local" "$TEST_ROOT/bin/spool-local"
printf '[database]\npath = "%s"\n' "$TEST_ROOT/spool.db" > "$TEST_ROOT/.spool/config.toml"
: > "$TEST_ROOT/Cargo.toml"

cat > "$TEST_ROOT/fake-bin/cargo" <<'FAKE'
#!/usr/bin/env sh
{
  pwd
  printf '%s\n' "$@"
} > "$SPOOL_LOCAL_CAPTURE"
FAKE
chmod +x "$TEST_ROOT/fake-bin/cargo"

CAPTURE="$TEST_ROOT/args.txt"
PATH="$TEST_ROOT/fake-bin:$PATH" SPOOL_LOCAL_CAPTURE="$CAPTURE" "$TEST_ROOT/bin/spool-local" queue list --format json 'space value'

EXPECTED="$TEST_ROOT/expected.txt"
cat > "$EXPECTED" <<EOF_EXPECTED
$TEST_ROOT
run
--manifest-path
$TEST_ROOT/Cargo.toml
-p
spool-cli
--bin
spool
--
--config
$TEST_ROOT/.spool/config.toml
queue
list
--format
json
space value
EOF_EXPECTED

diff -u "$EXPECTED" "$CAPTURE"

rm "$TEST_ROOT/.spool/config.toml"
if PATH="$TEST_ROOT/fake-bin:$PATH" "$TEST_ROOT/bin/spool-local" status >"$TEST_ROOT/missing.out" 2>"$TEST_ROOT/missing.err"; then
  echo "expected missing config failure" >&2
  exit 1
fi
if ! grep -q "expected project Spool config not found" "$TEST_ROOT/missing.err"; then
  echo "missing config error was not clear" >&2
  cat "$TEST_ROOT/missing.err" >&2
  exit 1
fi
