#!/usr/bin/env bash
# Build iMeme and deploy it to NEAR testnet. Run from the repo root.
#   NAME=imeme-yourname bash scripts/deploy.sh
# Creates three testnet accounts with the free faucet (keys saved to ~/.near-credentials):
#   $NAME.testnet (contract), brain-$NAME.testnet (the brain), player-$NAME.testnet (demo player)
set -euo pipefail
: "${NAME:?Set NAME, e.g. NAME=imeme-anas}"
OWNER="${OWNER:-$NAME.testnet}"   # owner can only change the brain account
C="$NAME.testnet"; B="brain-$NAME.testnet"; P="player-$NAME.testnet"

command -v cargo >/dev/null || { curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; . "$HOME/.cargo/env"; }
command -v near >/dev/null || curl --proto '=https' --tlsv1.2 -LsSf https://github.com/near/near-cli-rs/releases/latest/download/near-cli-rs-installer.sh | sh
command -v cargo-near >/dev/null || curl --proto '=https' --tlsv1.2 -LsSf https://github.com/near/cargo-near/releases/latest/download/cargo-near-installer.sh | sh
export PATH="$HOME/.cargo/bin:$PATH"

(cd contract && cargo test --quiet && cargo near build non-reproducible-wasm)

for A in "$C" "$B" "$P"; do
  near account create-account sponsor-by-faucet-service "$A" autogenerate-new-keypair save-to-legacy-keychain network-config testnet create
done

near contract deploy "$C" use-file contract/target/near/imeme.wasm \
  with-init-call new json-args "{\"owner\":\"$OWNER\",\"brain\":\"$B\"}" \
  prepaid-gas '100 Tgas' attached-deposit '0 NEAR' \
  network-config testnet sign-with-legacy-keychain send

echo
echo "Deployed: https://testnet.nearblocks.io/address/$C"
echo "Next:  CONTRACT=$C BRAIN=$B node brain/brain.mjs run"
echo "Dashboard: web/index.html?contract=$C"
