#!/usr/bin/env bash
# Demo moves as the player account. Usage: NAME=imeme-yourname bash scripts/play.sh claim|prompt "text"|enter "text"
set -euo pipefail
: "${NAME:?Set NAME}"; C="$NAME.testnet"; P="player-$NAME.testnet"
call() { near contract call-function as-transaction "$C" "$1" json-args "$2" prepaid-gas "${3:-100 Tgas}" attached-deposit "${4:-0.01 NEAR}" sign-as "$P" network-config testnet sign-with-legacy-keychain send; }
case "${1:-}" in
  claim)  call claim '{}' ;;
  prompt) call prompt "$(node -e 'console.log(JSON.stringify({text:process.argv[1]}))' "$2")" ;;
  enter)  call enter_ritual "$(node -e 'console.log(JSON.stringify({text:process.argv[1]}))' "$2")" ;;
  stats)  near contract call-function as-read-only "$C" get_stats json-args '{}' network-config testnet now ;;
  *) echo "usage: play.sh claim | prompt \"text\" | enter \"text\" | stats" ;;
esac
