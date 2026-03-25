#!/usr/bin/env zsh

RALPH=1 while :; do cat ralph.md | claude --dangerously-skip-permissions ; sleep 5; done
