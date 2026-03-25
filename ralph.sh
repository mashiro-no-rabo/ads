#!/usr/bin/env zsh

while :; do cat ralph.md | RALPH=1 claude --dangerously-skip-permissions ; sleep 5; done
