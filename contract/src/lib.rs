//! iMeme: "the joke needs rules".
//!
//! A NEP-141 memecoin whose mechanics are enforced on-chain:
//! 1. Fair launch: no team allocation. Anyone claims a fixed amount once, until the cap is hit.
//! 2. Burn-to-prompt, settled with NEAR yield/resume: a prompt suspends on-chain until the
//!    meme's AI "brain" resumes it with an answer. Answered: half the fee is burned, half feeds
//!    the ritual pot. Not answered before the yield times out: the full fee is refunded
//!    automatically. The price rises with demand ("heat") and cools off hourly.
//! 3. Daily ritual: the brain opens a challenge, holders enter by paying an entry fee into the pot,
//!    the brain scores entries off-chain and settles on-chain. The best entry (score >= 60) wins
//!    the whole pot. No winner means the pot rolls over.
//!
//! Prompts and ritual entries can be paid two ways: direct calls (`prompt`, `enter_ritual`) or a
//! standard NEP-141 `ft_transfer_call` to this contract with a JSON `msg`.
//!
//! Testnet prototype for the NEAR Hackathon. Not audited.

use near_contract_standards::fungible_token::events::{FtBurn, FtMint};
use near_contract_standards::fungible_token::metadata::{
    FungibleTokenMetadata, FungibleTokenMetadataProvider, FT_METADATA_SPEC,
};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_contract_standards::fungible_token::{
    FungibleToken, FungibleTokenCore, FungibleTokenResolver,
};
use near_contract_standards::storage_management::{
    StorageBalance, StorageBalanceBounds, StorageManagement,
};
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{U128, U64};
use near_sdk::serde_json;
use near_sdk::store::{LazyOption, LookupMap, LookupSet, Vector};
use near_sdk::{
    env, log, near, require, AccountId, BorshStorageKey, Gas, GasWeight, NearToken,
    PanicOnDefault, Promise, PromiseError, PromiseOrValue, YieldId,
};

pub const DECIMALS: u8 = 18;
pub const ONE: u128 = 1_000_000_000_000_000_000; // 10^18
pub const CLAIM_AMOUNT: u128 = 1_000 * ONE;
pub const MAX_CLAIMS: u32 = 1_000; // max supply 1,000,000 IMEME
pub const BASE_PROMPT_COST: u128 = 10 * ONE;
pub const HEAT_DECAY_NS: u64 = 3_600_000_000_000; // 1 hour
pub const MAX_PROMPT_LEN: usize = 280;
pub const MAX_ANSWER_LEN: usize = 1_000;
pub const MAX_CHALLENGE_LEN: usize = 280;
pub const PASS_SCORE: u8 = 60;
const GAS_FOR_ANSWER_CALLBACK: Gas = Gas::from_tgas(15);

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Token,
    Metadata,
    Claimed,
    Prompts,
    Rituals,
    Entries,
    Entered,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, PartialEq, Debug)]
pub enum PromptStatus {
    Pending,
    Answered,
    Expired,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Prompt {
    pub id: u32,
    pub author: AccountId,
    pub text: String,
    /// Price at the time of asking.
    pub cost: U128,
    /// Tokens held in escrow for this prompt (cost plus any overpayment).
    pub paid: U128,
    pub created_ns: U64,
    pub status: PromptStatus,
    pub answer: Option<String>,
    pub yield_id: Option<YieldId>,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Ritual {
    pub id: u32,
    pub challenge: String,
    pub entry_cost: U128,
    pub opens_ns: U64,
    pub closes_ns: U64,
    pub entries: u32,
    pub settled: bool,
    pub winner: Option<AccountId>,
    pub prize: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Entry {
    pub author: AccountId,
    pub text: String,
    pub score: Option<u8>,
    pub flagged: bool,
}

#[near(serializers = [json])]
pub struct Score {
    pub index: u32,
    pub score: u8,
    pub flagged: bool,
}

/// `msg` for `ft_transfer_call` to this contract:
/// `{"prompt":{"text":"gm"}}` or `{"enter":{"text":"my entry"}}`.
#[near(serializers = [json])]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Prompt { text: String },
    Enter { text: String },
}

#[near(serializers = [json])]
pub struct Stats {
    pub total_supply: U128,
    pub total_burned: U128,
    pub pot: U128,
    pub escrowed: U128,
    pub claims: u32,
    pub claims_left: u32,
    pub prompts: u32,
    pub rituals: u32,
    pub heat: u32,
    pub prompt_cost: U128,
    pub brain: AccountId,
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    owner: AccountId,
    brain: AccountId,
    token: FungibleToken,
    metadata: LazyOption<FungibleTokenMetadata>,
    claimed: LookupSet<AccountId>,
    claims: u32,
    total_burned: u128,
    heat: u32,
    heat_updated_ns: u64,
    /// Tokens the contract holds for pending prompts. Never part of the pot.
    escrowed: u128,
    /// Refunds promised to `ft_transfer_call` senders, paid out by `ft_resolve_transfer`.
    pending_refunds: u128,
    prompts: Vector<Prompt>,
    rituals: Vector<Ritual>,
    entries: LookupMap<(u32, u32), Entry>,
    entered: LookupSet<(u32, AccountId)>,
}

/// Charges the caller for the storage this call added and refunds the rest of the deposit.
fn charge_storage(initial_usage: u64) {
    let added = env::storage_usage().saturating_sub(initial_usage);
    let cost = env::storage_byte_cost().saturating_mul(added as u128);
    let attached = env::attached_deposit();
    require!(
        attached >= cost,
        format!(
            "Attach at least {} yoctoNEAR to cover storage",
            cost.as_yoctonear()
        )
    );
    let refund = attached.saturating_sub(cost);
    if refund > NearToken::from_yoctonear(0) {
        let _ = Promise::new(env::predecessor_account_id()).transfer(refund);
    }
}

#[near]
impl Contract {
    #[init]
    pub fn new(owner: AccountId, brain: AccountId) -> Self {
        let metadata = FungibleTokenMetadata {
            spec: FT_METADATA_SPEC.to_string(),
            name: "iMeme".to_string(),
            symbol: "IMEME".to_string(),
            icon: None,
            reference: None,
            reference_hash: None,
            decimals: DECIMALS,
        };
        let mut this = Self {
            owner,
            brain,
            token: FungibleToken::new(StorageKey::Token),
            metadata: LazyOption::new(StorageKey::Metadata, Some(metadata)),
            claimed: LookupSet::new(StorageKey::Claimed),
            claims: 0,
            total_burned: 0,
            heat: 0,
            heat_updated_ns: env::block_timestamp(),
            escrowed: 0,
            pending_refunds: 0,
            prompts: Vector::new(StorageKey::Prompts),
            rituals: Vector::new(StorageKey::Rituals),
            entries: LookupMap::new(StorageKey::Entries),
            entered: LookupSet::new(StorageKey::Entered),
        };
        // The contract itself holds escrow and the ritual pot.
        this.token
            .internal_register_account(&env::current_account_id());
        this
    }

    // ---------- 1. Fair launch ----------

    /// Claim the fair-launch allocation once per account. Attach a little NEAR for storage
    /// (0.01 NEAR is plenty; the unused part is refunded).
    #[payable]
    pub fn claim(&mut self) -> U128 {
        let initial = env::storage_usage();
        let who = env::predecessor_account_id();
        require!(self.claims < MAX_CLAIMS, "Fair launch is over: all claims are taken");
        require!(!self.claimed.contains(&who), "Already claimed");
        self.claimed.insert(who.clone());
        if !self.token.accounts.contains_key(&who) {
            self.token.internal_register_account(&who);
        }
        self.token.internal_deposit(&who, CLAIM_AMOUNT);
        self.claims += 1;
        FtMint {
            owner_id: &who,
            amount: U128(CLAIM_AMOUNT),
            memo: Some("fair-launch claim"),
        }
        .emit();
        charge_storage(initial);
        U128(CLAIM_AMOUNT)
    }

    // ---------- 2. Burn-to-prompt (yield/resume) ----------

    fn current_heat(&self) -> u32 {
        let elapsed = env::block_timestamp().saturating_sub(self.heat_updated_ns);
        let cooled = (elapsed / HEAT_DECAY_NS) as u32;
        self.heat.saturating_sub(cooled)
    }

    /// Price of the next prompt: +10% per unit of heat. Each prompt adds 1 heat;
    /// heat cools by 1 per hour.
    pub fn prompt_cost(&self) -> U128 {
        U128(BASE_PROMPT_COST * (10 + self.current_heat() as u128) / 10)
    }

    fn pot_amount(&self) -> u128 {
        self.token
            .ft_balance_of(env::current_account_id())
            .0
            .saturating_sub(self.escrowed + self.pending_refunds)
    }

    fn check_text(text: &str, max: usize, what: &str) {
        require!(!text.trim().is_empty(), format!("Empty {what}"));
        require!(
            text.chars().count() <= max,
            format!("{what} too long (max {max} chars)")
        );
    }

    /// Records a prompt whose `paid` tokens are already held by the contract, and suspends
    /// on a yield that the brain resumes with its answer.
    fn start_prompt(&mut self, author: AccountId, text: String, paid: u128, via_ft: bool) -> Promise {
        Self::check_text(&text, MAX_PROMPT_LEN, "prompt");
        let cost = self.prompt_cost().0;
        require!(paid >= cost, "Not enough IMEME for this prompt");
        self.escrowed += paid;
        self.heat = self.current_heat().saturating_add(1);
        self.heat_updated_ns = env::block_timestamp();

        let id = self.prompts.len();
        let callback = if via_ft { "on_answer_ft" } else { "on_answer" };
        let args = serde_json::to_vec(&serde_json::json!({ "prompt_id": id })).unwrap();
        let (promise, yield_id) =
            Promise::new_yield(callback, args, GAS_FOR_ANSWER_CALLBACK, GasWeight(0));
        self.prompts.push(Prompt {
            id,
            author,
            text,
            cost: U128(cost),
            paid: U128(paid),
            created_ns: U64(env::block_timestamp()),
            status: PromptStatus::Pending,
            answer: None,
            yield_id: Some(yield_id),
        });
        promise
    }

    /// Ask the brain something. Costs `prompt_cost()` tokens, held in escrow until the brain
    /// answers (half burned, half to the pot) or the yield times out (full refund).
    /// The transaction resolves with the brain's answer. Attach a little NEAR for storage
    /// (0.01 NEAR is plenty; the unused part is refunded).
    #[payable]
    pub fn prompt(&mut self, text: String) -> Promise {
        let initial = env::storage_usage();
        let who = env::predecessor_account_id();
        let cost = self.prompt_cost().0;
        require!(
            self.token.ft_balance_of(who.clone()).0 >= cost,
            "Not enough IMEME for this prompt"
        );
        self.token.internal_transfer(
            &who,
            &env::current_account_id(),
            cost,
            Some("prompt escrow".to_string()),
        );
        let promise = self.start_prompt(who, text, cost, false);
        charge_storage(initial);
        promise
    }

    /// Brain only: resume a pending prompt with its answer.
    pub fn answer_prompt(&mut self, id: u32, answer: String) {
        self.assert_brain();
        Self::check_text(&answer, MAX_ANSWER_LEN, "answer");
        let p = self.prompts.get(id).unwrap_or_else(|| env::panic_str("No such prompt"));
        require!(p.status == PromptStatus::Pending, "Prompt is not pending");
        let yield_id = p.yield_id.unwrap();
        let data = serde_json::to_vec(&answer).unwrap();
        if yield_id.resume(data).is_err() {
            env::panic_str("Too late: this prompt timed out and will be refunded");
        }
    }

    /// Settles a prompt after the yield resumes or times out. Returns the refund amount.
    fn settle_prompt(&mut self, prompt_id: u32, answer: Result<String, PromiseError>) -> u128 {
        let me = env::current_account_id();
        let (paid, cost, author) = {
            let p = self.prompts.get(prompt_id).unwrap();
            (p.paid.0, p.cost.0, p.author.clone())
        };
        self.escrowed = self.escrowed.saturating_sub(paid);
        let p = self.prompts.get_mut(prompt_id).unwrap();
        p.yield_id = None;
        match answer {
            Ok(text) => {
                p.status = PromptStatus::Answered;
                p.answer = Some(text);
                let burned = cost / 2;
                self.token.internal_withdraw(&me, burned);
                self.total_burned += burned;
                FtBurn {
                    owner_id: &me,
                    amount: U128(burned),
                    memo: Some("burn-to-prompt"),
                }
                .emit();
                log!("Prompt {} by {} answered; {} burned", prompt_id, author, burned);
                // The other half stays with the contract and becomes pot.
                paid - cost
            }
            Err(_) => {
                p.status = PromptStatus::Expired;
                log!("Prompt {} by {} timed out; refunding {}", prompt_id, author, paid);
                paid
            }
        }
    }

    /// Yield callback for prompts made with `prompt()`. Returns the brain's answer.
    #[private]
    pub fn on_answer(
        &mut self,
        prompt_id: u32,
        #[callback_result] answer: Result<String, PromiseError>,
    ) -> Option<String> {
        let refund = self.settle_prompt(prompt_id, answer);
        let author = self.prompts.get(prompt_id).unwrap().author.clone();
        if refund > 0 {
            self.token.internal_transfer(
                &env::current_account_id(),
                &author,
                refund,
                Some("prompt refund".to_string()),
            );
        }
        self.prompts.get(prompt_id).unwrap().answer.clone()
    }

    /// Yield callback for prompts paid with `ft_transfer_call`. Returns the unused amount,
    /// which `ft_resolve_transfer` refunds to the sender.
    #[private]
    pub fn on_answer_ft(
        &mut self,
        prompt_id: u32,
        #[callback_result] answer: Result<String, PromiseError>,
    ) -> U128 {
        let refund = self.settle_prompt(prompt_id, answer);
        self.pending_refunds += refund;
        U128(refund)
    }

    // ---------- 3. Daily ritual ----------

    fn active_ritual_index(&self) -> Option<u32> {
        let n = self.rituals.len();
        if n == 0 {
            return None;
        }
        if self.rituals.get(n - 1).unwrap().settled {
            None
        } else {
            Some(n - 1)
        }
    }

    /// Brain only: open a new ritual. Only one can be unsettled at a time.
    #[payable]
    pub fn open_ritual(&mut self, challenge: String, entry_cost: U128, duration_minutes: u32) -> u32 {
        let initial = env::storage_usage();
        self.assert_brain();
        require!(self.active_ritual_index().is_none(), "Settle the current ritual first");
        Self::check_text(&challenge, MAX_CHALLENGE_LEN, "challenge");
        require!(duration_minutes > 0 && duration_minutes <= 7 * 24 * 60, "Bad duration");
        let now = env::block_timestamp();
        let id = self.rituals.len();
        self.rituals.push(Ritual {
            id,
            challenge,
            entry_cost,
            opens_ns: U64(now),
            closes_ns: U64(now + duration_minutes as u64 * 60_000_000_000),
            entries: 0,
            settled: false,
            winner: None,
            prize: U128(0),
        });
        charge_storage(initial);
        id
    }

    /// Records an entry in the open ritual and returns its entry cost.
    /// Does not move tokens: callers handle payment.
    fn record_entry(&mut self, who: AccountId, text: String) -> (u32, u128) {
        let rid = self
            .active_ritual_index()
            .unwrap_or_else(|| env::panic_str("No ritual is open"));
        let (closes, cost) = {
            let r = self.rituals.get(rid).unwrap();
            (r.closes_ns.0, r.entry_cost.0)
        };
        require!(env::block_timestamp() < closes, "This ritual is closed for entries");
        Self::check_text(&text, MAX_PROMPT_LEN, "entry");
        require!(!self.entered.contains(&(rid, who.clone())), "Already entered");
        self.entered.insert((rid, who.clone()));
        let r = self.rituals.get_mut(rid).unwrap();
        let index = r.entries;
        r.entries += 1;
        self.entries.insert(
            (rid, index),
            Entry {
                author: who,
                text,
                score: None,
                flagged: false,
            },
        );
        (index, cost)
    }

    /// Enter the open ritual. Pays the entry fee into the pot. One entry per account.
    #[payable]
    pub fn enter_ritual(&mut self, text: String) -> u32 {
        let initial = env::storage_usage();
        let who = env::predecessor_account_id();
        let (index, cost) = self.record_entry(who.clone(), text);
        require!(
            self.token.ft_balance_of(who.clone()).0 >= cost,
            "Not enough IMEME for the entry fee"
        );
        if cost > 0 {
            self.token.internal_transfer(
                &who,
                &env::current_account_id(),
                cost,
                Some("ritual entry fee".to_string()),
            );
        }
        charge_storage(initial);
        index
    }

    /// Brain only, after the ritual closes: record scores and pay the pot to the best
    /// unflagged entry scoring at least 60. Ties go to the earlier entry.
    /// With no passing entry the pot rolls over to the next ritual.
    pub fn settle_ritual(&mut self, scores: Vec<Score>) -> Option<AccountId> {
        self.assert_brain();
        let rid = self
            .active_ritual_index()
            .unwrap_or_else(|| env::panic_str("No ritual to settle"));
        let (closes, n) = {
            let r = self.rituals.get(rid).unwrap();
            (r.closes_ns.0, r.entries)
        };
        require!(env::block_timestamp() >= closes, "Ritual still open");

        let mut best: Option<(u8, u32)> = None;
        for s in scores.iter() {
            require!(s.index < n, "Score for unknown entry");
            require!(s.score <= 100, "Score must be 0-100");
            let e = self.entries.get_mut(&(rid, s.index)).unwrap();
            require!(e.score.is_none(), "Entry scored twice");
            let score = if s.flagged { 0 } else { s.score };
            e.score = Some(score);
            e.flagged = s.flagged;
            if !s.flagged && score >= PASS_SCORE {
                match best {
                    Some((b, i)) if b > score || (b == score && i < s.index) => {}
                    _ => best = Some((score, s.index)),
                }
            }
        }

        let pot = self.pot_amount();
        let winner = best.map(|(_, i)| self.entries.get(&(rid, i)).unwrap().author.clone());
        if let Some(w) = winner.clone() {
            if pot > 0 {
                self.token.internal_transfer(
                    &env::current_account_id(),
                    &w,
                    pot,
                    Some("ritual prize".to_string()),
                );
            }
            log!("Ritual {} won by {} for {} yocto-IMEME", rid, w, pot);
        } else {
            log!("Ritual {}: no passing entry, pot of {} rolls over", rid, pot);
        }
        let r = self.rituals.get_mut(rid).unwrap();
        r.settled = true;
        r.winner = winner.clone();
        r.prize = U128(if winner.is_some() { pot } else { 0 });
        winner
    }

    // ---------- Admin ----------

    pub fn set_brain(&mut self, brain: AccountId) {
        require!(env::predecessor_account_id() == self.owner, "Owner only");
        self.brain = brain;
    }

    fn assert_brain(&self) {
        require!(env::predecessor_account_id() == self.brain, "Brain only");
    }

    // ---------- Views ----------

    pub fn get_stats(&self) -> Stats {
        Stats {
            total_supply: self.token.ft_total_supply(),
            total_burned: U128(self.total_burned),
            pot: U128(self.pot_amount()),
            escrowed: U128(self.escrowed),
            claims: self.claims,
            claims_left: MAX_CLAIMS - self.claims,
            prompts: self.prompts.len(),
            rituals: self.rituals.len(),
            heat: self.current_heat(),
            prompt_cost: self.prompt_cost(),
            brain: self.brain.clone(),
        }
    }

    pub fn has_claimed(&self, account_id: AccountId) -> bool {
        self.claimed.contains(&account_id)
    }

    pub fn get_prompt(&self, id: u32) -> Option<Prompt> {
        self.prompts.get(id).cloned()
    }

    /// Newest first.
    pub fn get_prompts(&self, from: Option<u32>, limit: Option<u32>) -> Vec<Prompt> {
        let n = self.prompts.len();
        let skip = from.unwrap_or(0).min(n);
        let limit = limit.unwrap_or(20).min(50);
        (0..n - skip)
            .rev()
            .take(limit as usize)
            .filter_map(|i| self.prompts.get(i).cloned())
            .collect()
    }

    /// Prompts waiting for the brain, oldest first. Scans the newest 200 prompts only.
    pub fn get_pending(&self, limit: Option<u32>) -> Vec<Prompt> {
        let limit = limit.unwrap_or(20).min(50) as usize;
        let n = self.prompts.len();
        (n.saturating_sub(200)..n)
            .filter_map(|i| self.prompts.get(i))
            .filter(|p| p.status == PromptStatus::Pending)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn get_ritual(&self, id: u32) -> Option<Ritual> {
        self.rituals.get(id).cloned()
    }

    pub fn get_current_ritual(&self) -> Option<Ritual> {
        let n = self.rituals.len();
        if n == 0 {
            None
        } else {
            self.rituals.get(n - 1).cloned()
        }
    }

    pub fn get_entries(&self, ritual_id: u32) -> Vec<Entry> {
        let n = self.rituals.get(ritual_id).map(|r| r.entries).unwrap_or(0);
        (0..n.min(100))
            .filter_map(|i| self.entries.get(&(ritual_id, i)).cloned())
            .collect()
    }
}

// ---------- NEP-141 receiver: pay with ft_transfer_call ----------

#[near]
impl FungibleTokenReceiver for Contract {
    /// Only IMEME itself is accepted. `msg` is an `Action` in JSON.
    /// Storage for the stored text is paid by the contract's own NEAR balance.
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Only IMEME is accepted"
        );
        let action: Action = serde_json::from_str(&msg)
            .unwrap_or_else(|_| env::panic_str("Bad msg: use {\"prompt\":{\"text\":..}} or {\"enter\":{\"text\":..}}"));
        match action {
            Action::Prompt { text } => {
                PromiseOrValue::Promise(self.start_prompt(sender_id, text, amount.0, true))
            }
            Action::Enter { text } => {
                let (_, cost) = self.record_entry(sender_id, text);
                require!(amount.0 >= cost, "Not enough IMEME for the entry fee");
                let unused = amount.0 - cost;
                self.pending_refunds += unused;
                PromiseOrValue::Value(U128(unused))
            }
        }
    }
}

// ---------- NEP-141 plumbing ----------

#[near]
impl FungibleTokenCore for Contract {
    #[payable]
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>) {
        self.token.ft_transfer(receiver_id, amount, memo)
    }

    #[payable]
    fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<U128> {
        self.token.ft_transfer_call(receiver_id, amount, memo, msg)
    }

    fn ft_total_supply(&self) -> U128 {
        self.token.ft_total_supply()
    }

    fn ft_balance_of(&self, account_id: AccountId) -> U128 {
        self.token.ft_balance_of(account_id)
    }
}

#[near]
impl FungibleTokenResolver for Contract {
    #[private]
    fn ft_resolve_transfer(
        &mut self,
        sender_id: AccountId,
        receiver_id: AccountId,
        amount: U128,
    ) -> U128 {
        // A transfer_call to ourselves may have booked a refund in `pending_refunds`.
        // Release exactly what ft_on_transfer (or its yield callback) reported as unused.
        if receiver_id == env::current_account_id() {
            if let Ok(bytes) = env::promise_result_checked(0, 64) {
                if let Ok(U128(unused)) = serde_json::from_slice::<U128>(&bytes) {
                    self.pending_refunds = self.pending_refunds.saturating_sub(unused.min(amount.0));
                }
            }
        }
        let (used, burned) =
            self.token
                .internal_ft_resolve_transfer(&sender_id, receiver_id, amount);
        if burned > 0 {
            self.total_burned += burned;
        }
        used.into()
    }
}

#[near]
impl StorageManagement for Contract {
    #[payable]
    fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        registration_only: Option<bool>,
    ) -> StorageBalance {
        self.token.storage_deposit(account_id, registration_only)
    }

    #[payable]
    fn storage_withdraw(&mut self, amount: Option<NearToken>) -> StorageBalance {
        self.token.storage_withdraw(amount)
    }

    #[payable]
    fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        // Force-unregister would silently burn tokens; disallow to keep supply accounting honest.
        require!(force != Some(true), "Force unregister is disabled");
        self.token.internal_storage_unregister(force).is_some()
    }

    fn storage_balance_bounds(&self) -> StorageBalanceBounds {
        self.token.storage_balance_bounds()
    }

    fn storage_balance_of(&self, account_id: AccountId) -> Option<StorageBalance> {
        self.token.storage_balance_of(account_id)
    }
}

#[near]
impl FungibleTokenMetadataProvider for Contract {
    fn ft_metadata(&self) -> FungibleTokenMetadata {
        self.metadata.get().clone().unwrap()
    }
}

#[cfg(test)]
mod tests;
