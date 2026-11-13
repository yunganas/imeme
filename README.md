# iMeme: the joke needs rules

A memecoin with a pluggable "brain" (scripted persona by default, optional LLM) and mechanics the NEAR chain enforces. Built at NEAR HACKATHON for the **Memecoins With Mechanics** track. Testnet prototype, not audited.

**Live dashboard:** [yunganas.github.io/imeme/web](https://yunganas.github.io/imeme/web/?contract=imeme-anas.testnet) (restyled Nov 13, 2026: new look and clearer claim-rule wording, no logic or contract changes; the demo video shows the earlier style)
**Contract (testnet):** `imeme-anas.testnet`

## The mechanics

1. **Fair launch.** No team allocation and no owner or admin mint. Any account calls `claim()` once for 1,000 IMEME. The first 1,000 claims cap supply at 1,000,000.
2. **Burn-to-prompt, settled with yield/resume.** `prompt(text)` moves the fee into on-chain escrow and suspends the transaction with `Promise::new_yield`. The brain account resumes it with `answer_prompt`. If answered, half the fee is **burned** (NEP-141 `ft_burn` event) and half goes to the ritual pot. If the yield times out, the contract **refunds the full fee automatically**. The brain can never keep a fee it didn't earn.
3. **Heat pricing.** The prompt price starts at 10 IMEME and rises 10% per unit of heat. Each prompt adds 1 heat, and heat cools by 1 per hour.
4. **Rituals.** The brain opens a challenge with an entry fee. Holders enter once each, and fees go into the pot. After close, the brain posts scores on-chain. The best unflagged entry scoring 60 or more wins the whole pot, and ties go to the earlier entry. With no passing entry, the pot rolls over.
5. **Standard payments.** Prompts and entries can also be paid with NEP-141 `ft_transfer_call` (`{"prompt":{"text":"gm"}}` or `{"enter":{"text":"..."}}`). Unused amounts are returned through `ft_resolve_transfer`.

## NEAR features used

- **Yield/resume** (`Promise::new_yield`, `YieldId::resume`) for escrowed, refund-on-timeout brain answers.
- **NEP-141** fungible token with `ft_transfer_call`, burn and mint events (near-contract-standards 5.27).
- **NEP-145** storage management. Callers pay their own storage, and unused deposit is refunded.

## Repo layout

| Path | What |
|---|---|
| `contract/` | Rust smart contract (near-sdk 5.27.0, Rust 1.86). `cargo test` runs 20 unit tests. |
| `brain/brain.mjs` | The brain. It polls pending prompts, answers before timeout, and settles rituals. Has no npm dependencies and signs with `near` CLI. |
| `web/index.html` | Live read-only dashboard over JSON-RPC: supply, burned, pot, heat, prompt feed, ritual. |
| `scripts/deploy.sh` | Builds, creates testnet accounts with the faucet, and deploys. |
| `scripts/play.sh` | Demo moves as a player: claim, prompt, enter, stats. |

## Run it yourself

Needs Node 18+. `deploy.sh` installs Rust, the near CLI and cargo-near. When the near CLI asks for a key scheme, choose **ed25519**.

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"            # in every new terminal
NAME=imeme-yourname bash scripts/deploy.sh                          # build + deploy on testnet
CONTRACT=imeme-yourname.testnet BRAIN=brain-imeme-yourname.testnet node brain/brain.mjs run
NAME=imeme-yourname bash scripts/play.sh claim
NAME=imeme-yourname bash scripts/play.sh prompt "gm iMeme, are you a rug?"
CONTRACT=... BRAIN=... node brain/brain.mjs ritual "Best one-line roast of gas fees" 50 30
```

The brain answers in a built-in persona by default. To use an LLM instead, set `LLM_BASE_URL`, `LLM_API_KEY` and `LLM_MODEL` for any OpenAI-compatible endpoint. With no LLM, ritual scoring uses a transparent heuristic (length plus on-topic words, with duplicates flagged).

## Honest limits

- The brain is a trusted account. It decides answers and ritual scores off-chain. The contract limits what it can do: it can't mint, can't take escrow without answering, and can only pay the pot to an entrant.
- Storage for `ft_transfer_call` prompts comes from the contract's own NEAR balance.
- Testnet only. Not audited. Not financial advice.
- Exercised on testnet so far: claim, burn-to-prompt, the timeout refund, a winning ritual and a rollover. Not yet exercised there: `ft_transfer_call` payments, hourly heat cooling, duplicate flagging.

## Known issues (found 2026-11-13, not yet fixed)

A manual review of `contract/src/lib.rs` found these. It is not a professional audit. Testnet only. Fixes are planned after hackathon judging so the testnet history shown in the demo stays intact.

- **"Fair launch" is first come, one claim per account.** It is not Sybil-resistant: one person can claim with many accounts.
- **Rituals have no entry cap (medium).** `get_entries` returns only the first 100, so later entrants pay but can't be scored. Planned fix: cap entries at 100.
- **The brain can rig a ritual (medium).** Nothing stops the brain or owner from entering, and the brain may score only some entries. Planned fix: block brain and owner entries and require every entry to be scored.
- **`ft_transfer_call` prompts are cheap to spam (medium).** Their storage comes from the contract's NEAR and they are fully refunded on timeout. Planned fix: remove that path and keep `prompt()`, which charges the caller for storage.
- **Owner powers.** The owner can replace the brain with `set_brain` and still holds a full-access key on the contract account.

## Also by this author

This is one of two independent projects I entered in the NEAR Hackathon. The other is **[Swarm-for-Hire](https://github.com/yunganas/near-swarm)**, an agent job marketplace. The two share no code.

## License

MIT
