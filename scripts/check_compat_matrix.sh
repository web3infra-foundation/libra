#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

CLI_COMMANDS="$(mktemp)"
MATRIX_COMMANDS="$(mktemp)"
MISSING_COMMANDS="$(mktemp)"
EXTRA_COMMANDS="$(mktemp)"
trap 'rm -f "$CLI_COMMANDS" "$MATRIX_COMMANDS" "$MISSING_COMMANDS" "$EXTRA_COMMANDS"' EXIT

awk '
function kebab(name, out, i, ch, prev) {
  out = ""
  prev = ""
  for (i = 1; i <= length(name); i++) {
    ch = substr(name, i, 1)
    if (ch ~ /[A-Z]/ && i > 1 && prev ~ /[a-z0-9]/) {
      out = out "-"
    }
    out = out tolower(ch)
    prev = ch
  }
  return out
}

/^enum Commands[[:space:]]*\{/ {
  in_commands = 1
  next
}

in_commands && /^}/ {
  exit
}

in_commands {
  line = $0
  sub(/^[[:space:]]+/, "", line)
  if (line ~ /^[A-Z][A-Za-z0-9]*[({]/) {
    name = line
    sub(/[({].*/, "", name)
    print kebab(name)
  }
}
' "$ROOT/src/cli.rs" | sort -u > "$CLI_COMMANDS"

awk '
/^## Top-level commands \(from `src\/cli.rs`\)/ {
  in_matrix = 1
  next
}

/^## Git commands intentionally absent from `src\/cli.rs`/ {
  in_matrix = 0
}

in_matrix && /^\|/ {
  split($0, cols, "|")
  command = cols[2]
  gsub(/^[[:space:]]+|[[:space:]]+$/, "", command)
  if (command == "" || command == "Command" || command ~ /^-+$/) {
    next
  }
  print command
}
' "$ROOT/COMPATIBILITY.md" | sort -u > "$MATRIX_COMMANDS"

comm -23 "$CLI_COMMANDS" "$MATRIX_COMMANDS" > "$MISSING_COMMANDS"
comm -13 "$CLI_COMMANDS" "$MATRIX_COMMANDS" > "$EXTRA_COMMANDS"

if [[ -s "$MISSING_COMMANDS" || -s "$EXTRA_COMMANDS" ]]; then
  echo "COMPATIBILITY.md top-level command matrix is out of sync with src/cli.rs::Commands." >&2
  if [[ -s "$MISSING_COMMANDS" ]]; then
    echo >&2
    echo "Missing from COMPATIBILITY.md:" >&2
    sed 's/^/  - /' "$MISSING_COMMANDS" >&2
  fi
  if [[ -s "$EXTRA_COMMANDS" ]]; then
    echo >&2
    echo "Listed in COMPATIBILITY.md but absent from src/cli.rs::Commands:" >&2
    sed 's/^/  - /' "$EXTRA_COMMANDS" >&2
  fi
  exit 1
fi

COUNT="$(wc -l < "$CLI_COMMANDS" | tr -d " ")"
echo "COMPATIBILITY.md top-level command matrix matches src/cli.rs::Commands (${COUNT} commands)."
