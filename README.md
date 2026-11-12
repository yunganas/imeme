# iMeme: the joke needs rules

A memecoin with an AI "brain" and mechanics the NEAR chain enforces. Built at NEAR HACKATHON for the **Memecoins With Mechanics** track. Testnet prototype, not audited.

**Live dashboard:** `https://yunganas.github.io/imeme/web/?contract=imeme-anas.testnet`
**Contract (testnet):** `imeme-anas.testnet`

## The mechanics

1. **Fair launch.** No team allocation and no mint function. Any account calls `claim()` once for 1,000 IMEME. The first 1,000 claims cap supply at 1,000,000.
2. **Burn-to-prompt, settled with yield/resume.** `prompt(text)` moves the fee into on-chain escrow and suspends the transaction with `Promise::new_yield`. The brain account resumes it with `answer_prompt`. If answered, half the fee is **burned** (NEP-141 `ft_burn` event) and half goes to the ritual pot. If the yield times out, the contract **refunds the full fee automatically**. The brain can never keep a fee it didn't earn.
3. **Heat pricing.** The prompt price starts at 10 IMEME and rises 10% per unit of heat. Each prompt adds 1 heat, and heat cools by 1 per hour.
4. **Rituals.** The brain opens a challenge with an entry fee. Holders enter once each, and fees go into the pot. After close, the brain posts scores on-chain. The best unflagged entry scoring 60 or more wins the whole pot, and ties go to the earlier entry. With no passing entry, the pot rolls over.
5. **Standard payments.** Prompts and entries can also be paid with NEP-141 `ft_transfer_call` (`{"prompt":{"text":"gm"}}` or `{"enter":{"text":"..."}}`). Unused amounts are returned through `ft_resolve_transfer`.

## NEAR features used

- **Yield/resume** (`Promise::new_yield`, `YieldId::resume`) for escrowed, refund-on-timeout AI answers.
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

```bash
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

## Also by this author

This is one of two independent projects I entered in the NEAR Hackathon. The other is **[Swarm-for-Hire](https://github.com/yunganas/near-swarm)**, an agent job marketplace. The two share no code.

## License

MIT
