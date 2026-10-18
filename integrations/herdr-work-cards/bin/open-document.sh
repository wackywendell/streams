#!/bin/sh
# The prefix is trusted global user configuration; the path is never shell source.
fail() {
  printf '\nWork Cards document command failed (%s). Check document_command / EDITOR and the checkout path.\nPress Enter to close this tab.\n' "$1" >&2
  IFS= read -r answer
  exit "$1"
}

cd "$WORK_CARDS_DOCUMENT_CWD" || fail 1
/bin/sh -c "$WORK_CARDS_DOCUMENT_COMMAND"' "$1"' work-cards-document "$WORK_CARDS_DOCUMENT_PATH"
status=$?
[ "$status" -eq 0 ] || fail "$status"
