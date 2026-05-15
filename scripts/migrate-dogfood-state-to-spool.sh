#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
SOURCE_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd -P)

GIT_COMMON_DIR=$(cd "$SOURCE_ROOT" && git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
if [ -n "$GIT_COMMON_DIR" ] && [ -d "$(dirname -- "$GIT_COMMON_DIR")/.tasker" ]; then
  MANAGED_ROOT=$(CDPATH= cd -- "$(dirname -- "$GIT_COMMON_DIR")" && pwd -P)
else
  MANAGED_ROOT="$SOURCE_ROOT"
fi

OLD_STATE_DIR="$MANAGED_ROOT/.tasker"
OLD_DB="$OLD_STATE_DIR/data/tasker.db"
NEW_STATE_DIR="$MANAGED_ROOT/.spool"
NEW_DATA_DIR="$NEW_STATE_DIR/data"
NEW_DB="$NEW_DATA_DIR/spool.db"
NEW_CONFIG="$NEW_STATE_DIR/config.toml"
BACKUP_SUFFIX=$(date +%Y%m%d%H%M%S)

if [ ! -f "$OLD_DB" ]; then
  echo "error: old Tasker database not found at $OLD_DB" >&2
  exit 78
fi

mkdir -p "$NEW_DATA_DIR" "$NEW_STATE_DIR/worktrees"

if [ -f "$NEW_DB" ]; then
  cp "$NEW_DB" "$NEW_DB.before-tasker-to-spool-$BACKUP_SUFFIX"
fi

if [ -f "$NEW_CONFIG" ]; then
  cp "$NEW_CONFIG" "$NEW_CONFIG.before-tasker-to-spool-$BACKUP_SUFFIX"
fi

sqlite3 "$OLD_DB" ".backup '$NEW_DB'"

cat > "$NEW_CONFIG" <<EOF
[service]
bind_addr = "127.0.0.1:4317"

[database]
path = "$NEW_DB"
EOF

if [ -d "$OLD_STATE_DIR/data/runs" ]; then
  mkdir -p "$NEW_DATA_DIR/runs"
  (cd "$OLD_STATE_DIR/data/runs" && tar cf - .) | (cd "$NEW_DATA_DIR/runs" && tar xpf -)
fi

if [ -d "$OLD_STATE_DIR/data/supervisors" ]; then
  mkdir -p "$NEW_DATA_DIR/supervisors"
  (cd "$OLD_STATE_DIR/data/supervisors" && tar cf - .) | (cd "$NEW_DATA_DIR/supervisors" && tar xpf -)
  if [ -f "$NEW_DATA_DIR/supervisors/TASKER.lock" ] && [ ! -f "$NEW_DATA_DIR/supervisors/SPOOL.lock" ]; then
    cp "$NEW_DATA_DIR/supervisors/TASKER.lock" "$NEW_DATA_DIR/supervisors/SPOOL.lock"
  fi
fi

if [ -f "$OLD_STATE_DIR/validation-commands.txt" ] && [ ! -f "$NEW_STATE_DIR/validation-commands.txt" ]; then
  cp "$OLD_STATE_DIR/validation-commands.txt" "$NEW_STATE_DIR/validation-commands.txt"
fi

(cd "$SOURCE_ROOT" && cargo run -q -p spool-cli --bin spool -- \
  --config "$NEW_CONFIG" \
  --data-dir "$NEW_DATA_DIR" \
  db migrate --allow-task-branch)

sqlite3 "$NEW_DB" <<EOF
BEGIN;
UPDATE task_queues
SET key = 'SPOOL',
    name = 'Spool Dogfood',
    worktree_root = '$NEW_STATE_DIR/worktrees',
    branch_template = 'spool/{task_identifier}',
    updated_at = CURRENT_TIMESTAMP
WHERE key = 'TASKER';
COMMIT;
EOF

echo "Dogfood local state migrated to Spool"
echo "managed source repository: $MANAGED_ROOT"
echo "config: $NEW_CONFIG"
echo "data: $NEW_DATA_DIR"
echo "database: $NEW_DB"
echo "preflight: bin/spool-local queue show SPOOL"
