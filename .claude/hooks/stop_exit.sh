#!/usr/bin/env bash

if [ -z "$RALPH" ]; then
  exit 0
fi

pid=$PPID
while [ -n "$pid" ] && [ "$pid" -gt 1 ]; do
    name=$(ps -p "$pid" -o comm= 2>/dev/null)
    if [ "$name" = "claude" ]; then
        (sleep 2 && kill "$pid") &
        break
    fi
    pid=$(ps -p "$pid" -o ppid= 2>/dev/null | tr -d ' ')
done

jq -n '{"continue": false, "stopReason": "Exiting..."}'
