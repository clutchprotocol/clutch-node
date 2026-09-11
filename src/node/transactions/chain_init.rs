use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use super::address::canonical_account_address;

pub const CHAIN_PARAMS_KEY: &[u8] = b"chain_params";
pub const TOTAL_SUPPLY_KEY: &[u8] = b"total_supply";

/// Consensus parameters, committed to by the genesis hash: this struct rides in the
/// genesis block's single ChainInit transaction, whose hash feeds the block hash that
/// peers compare at p2p handshake. Runtime reads them from state via `get`, never from
/// per-node config — a node with different values gets a different genesis and cannot
/// peer. This closes the block_reward-style consensus-divergence bug class.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChainInit {
    pub chain_id: u64,
    pub is_testnet: bool,
    pub tx_fee: u64,
    pub ride_request_referrer_fee_bps: u16,
    pub ride_offer_referrer_fee_bps: u16,
    pub mint_authority: String,
    pub faucet_address: String,
    pub faucet_allocation: u64,
    /// Further addresses authorised to sign a `Mint`, on top of `mint_authority`. The full
    /// authority set is `{mint_authority}` plus these. Empty is today's single-signer chain.
    #[serde(default)]
    pub mint_cosigners: Vec<String>,
    /// Signatures a `Mint` requires. `0` and `1` both mean single-signer.
    ///
    /// Above 1, a Mint must be submitted by one member of the authority set and carry
    /// `mint_threshold - 1` further approval signatures from distinct other members — so a
    /// stolen key mints nothing on its own. The point is that this is enforced by consensus:
    /// the four-eyes rule in `treasury-service` is off-chain and a stolen key walks past it.
    #[serde(default)]
    pub mint_threshold: u8,
    /// Seconds after a `RideAcceptance` before the held fare stops being the rider's to reclaim.
    /// `0` disables the rule entirely, which is every chain that existed before it.
    ///
    /// Genesis-committed because it decides who receives money. A node running a different value
    /// would compute a different balance from the same block — the consensus-divergence class this
    /// struct exists to prevent. That is also why it cannot be per-acceptance: a rider choosing
    /// their own window would choose one that never expires.
    ///
    /// Seconds rather than blocks, because block cadence is `60 / authority_count` and changing the
    /// validator set would silently change the window.
    #[serde(default)]
    pub ride_auto_release_secs: u64,
}

impl ChainInit {
    pub fn get(db: &Database) -> Result<ChainInit, String> {
        match db.get("state", CHAIN_PARAMS_KEY) {
            Ok(Some(v)) => serde_json::from_slice(&v)
                .map_err(|e| format!("corrupt chain_params in state: {}", e)),
            Ok(None) => Err("chain_params missing from state (genesis not imported?)".to_string()),
            Err(e) => Err(format!("failed to read chain_params: {}", e)),
        }
    }

    pub fn get_total_supply(db: &Database) -> Result<u64, String> {
        match db.get("state", TOTAL_SUPPLY_KEY) {
            Ok(Some(v)) => serde_json::from_slice(&v)
                .map_err(|e| format!("corrupt total_supply in state: {}", e)),
            Ok(None) => Ok(0),
            Err(e) => Err(format!("failed to read total_supply: {}", e)),
        }
    }

    /// Every address allowed to sign a `Mint`, canonicalised for comparison.
    pub fn mint_authority_set(&self) -> Vec<String> {
        let mut set = vec![canonical_account_address(&self.mint_authority)];
        for c in &self.mint_cosigners {
            let canon = canonical_account_address(c);
            if !set.contains(&canon) {
                set.push(canon);
            }
        }
        set
    }

    /// Signatures a Mint needs. Both `0` and `1` mean one, so a genesis that never set the field
    /// behaves exactly as before it existed.
    pub fn effective_mint_threshold(&self) -> usize {
        self.mint_threshold.max(1) as usize
    }

    /// True when the held fare auto-releases to the driver after a window.
    pub fn uses_auto_release(&self) -> bool {
        self.ride_auto_release_secs > 0
    }

    /// True when this chain is configured for multi-signature minting. Also decides whether the
    /// two fields appear in the RLP at all, which is what keeps a single-signer genesis hash
    /// byte-identical to one produced before these fields existed.
    pub fn uses_multisig_mint(&self) -> bool {
        self.mint_threshold > 1
    }

    pub fn verify_state(&self, _from: &String, _db: &Database) -> Result<(), String> {
        // Genesis import bypasses validate_transaction entirely, so reaching this check
        // means the tx arrived via the pool or a non-genesis block — always reject.
        Err("ChainInit is only valid in the genesis block".to_string())
    }

    pub fn state_transaction(&self, db: &Database) -> Vec<StateUpdate> {
        let initial_supply = if self.is_testnet { self.faucet_allocation } else { 0 };
        let mut updates = vec![
            StateUpdate::storage_only(
                CHAIN_PARAMS_KEY.to_vec(),
                serde_json::to_vec(self).expect("serialize chain params"),
            ),
            StateUpdate::storage_only(
                TOTAL_SUPPLY_KEY.to_vec(),
                serde_json::to_vec(&initial_supply).expect("serialize supply"),
            ),
        ];
        if initial_supply > 0 {
            // faucet_allocation is validated <= i64::MAX at boot (Blockchain::new).
            updates.push(AccountState::apply_balance_change(
                &self.faucet_address,
                initial_supply as i64,
                BalanceEffectKind::Mint,
                None,
                db,
            ));
        }
        updates
    }
}

impl Encodable for ChainInit {
    fn rlp_append(&self, stream: &mut RlpStream) {
        // 8 items unless multisig minting is configured, then 10. A single-signer chain therefore
        // encodes byte-identically to one produced before these fields existed, which is what lets
        // the running testnet keep its genesis hash while mainnet opts in.
        // 8, 10 or 11 items. Each step is additive and only appears when the feature is in use,
        // so a chain that uses neither encodes byte-identically to one from before either existed
        // and keeps its genesis hash. 11 carries the mint fields even at their defaults, because
        // a length alone has to say unambiguously which fields are present.
        let items = if self.uses_auto_release() {
            11
        } else if self.uses_multisig_mint() {
            10
        } else {
            8
        };
        stream.begin_list(items);
        stream.append(&self.chain_id);
        stream.append(&(self.is_testnet as u8));
        stream.append(&self.tx_fee);
        stream.append(&self.ride_request_referrer_fee_bps);
        stream.append(&self.ride_offer_referrer_fee_bps);
        stream.append(&self.mint_authority);
        stream.append(&self.faucet_address);
        stream.append(&self.faucet_allocation);
        if self.uses_multisig_mint() || self.uses_auto_release() {
            stream.append_list::<String, String>(&self.mint_cosigners);
            stream.append(&self.mint_threshold);
        }
        if self.uses_auto_release() {
            stream.append(&self.ride_auto_release_secs);
        }
    }
}

impl Decodable for ChainInit {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() {
            return Err(DecoderError::RlpExpectedToBeList);
        }
        // 8 = a chain from before multisig minting existed; 10 = one that configured it. Any other
        // length is a genesis this build cannot agree about, which must fail rather than guess.
        let (has_mint_fields, has_auto_release) = match rlp.item_count()? {
            8 => (false, false),
            10 => (true, false),
            11 => (true, true),
            _ => return Err(DecoderError::RlpIncorrectListLen),
        };
        let multisig = has_mint_fields;
        Ok(ChainInit {
            chain_id: rlp.val_at(0)?,
            is_testnet: rlp.val_at::<u8>(1)? != 0,
            tx_fee: rlp.val_at(2)?,
            ride_request_referrer_fee_bps: rlp.val_at(3)?,
            ride_offer_referrer_fee_bps: rlp.val_at(4)?,
            mint_authority: rlp.val_at(5)?,
            faucet_address: rlp.val_at(6)?,
            faucet_allocation: rlp.val_at(7)?,
            mint_cosigners: if multisig { rlp.list_at(8)? } else { Vec::new() },
            mint_threshold: if multisig { rlp.val_at(9)? } else { 0 },
            ride_auto_release_secs: if has_auto_release { rlp.val_at(10)? } else { 0 },
        })
    }
}

#[cfg(test)]
mod mint_authority_tests {
    use super::*;
    use rlp::Rlp;

    const A: &str = "0x00000000000000000000000000000000000000aa";
    const B: &str = "0x00000000000000000000000000000000000000bb";
    const C: &str = "0x00000000000000000000000000000000000000cc";

    fn params(cosigners: Vec<String>, threshold: u8) -> ChainInit {
        ChainInit {
            chain_id: 2077,
            is_testnet: true,
            tx_fee: 1000,
            ride_request_referrer_fee_bps: 200,
            ride_offer_referrer_fee_bps: 200,
            mint_authority: A.to_string(),
            faucet_address: "0x0000000000000000000000000000000000000000".to_string(),
            faucet_allocation: 0,
            mint_cosigners: cosigners,
            mint_threshold: threshold,
            ride_auto_release_secs: 0,
        }
    }

    #[test]
    fn threshold_zero_and_one_both_mean_one() {
        assert_eq!(params(vec![], 0).effective_mint_threshold(), 1);
        assert_eq!(params(vec![], 1).effective_mint_threshold(), 1);
        assert!(!params(vec![], 0).uses_multisig_mint());
        assert!(!params(vec![], 1).uses_multisig_mint());
        assert!(params(vec![B.to_string()], 2).uses_multisig_mint());
    }

    #[test]
    fn the_authority_set_includes_the_primary_and_dedupes() {
        let p = params(vec![B.to_string(), C.to_string()], 2);
        assert_eq!(p.mint_authority_set().len(), 3);

        // A cosigner repeating the primary must not inflate the set, or a 2-of-2 would be
        // satisfiable by the primary alone.
        let p = params(vec![A.to_uppercase(), B.to_string()], 2);
        assert_eq!(
            p.mint_authority_set().len(),
            2,
            "the same address in both fields is one authority"
        );
    }

    /// The compatibility guarantee the whole design rests on: a chain that does not use
    /// multi-signature minting must encode byte-identically to one produced before these fields
    /// existed, so the running testnet keeps its genesis hash.
    #[test]
    fn single_signer_chain_init_encodes_as_eight_items() {
        let encoded = rlp::encode(&params(vec![], 0));
        assert_eq!(Rlp::new(&encoded).item_count().unwrap(), 8);

        // And byte-for-byte against a hand-built legacy encoding.
        let mut legacy = RlpStream::new();
        legacy.begin_list(8);
        legacy.append(&2077u64);
        legacy.append(&1u8);
        legacy.append(&1000u64);
        legacy.append(&200u16);
        legacy.append(&200u16);
        legacy.append(&A.to_string());
        legacy.append(&"0x0000000000000000000000000000000000000000".to_string());
        legacy.append(&0u64);
        assert_eq!(
            encoded.to_vec(),
            legacy.out().to_vec(),
            "a single-signer genesis must hash exactly as it did before M-of-N existed"
        );
    }

    #[test]
    fn multisig_chain_init_round_trips() {
        let p = params(vec![B.to_string(), C.to_string()], 2);
        let encoded = rlp::encode(&p);
        assert_eq!(Rlp::new(&encoded).item_count().unwrap(), 10);
        let decoded: ChainInit = rlp::decode(&encoded).unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn single_signer_chain_init_round_trips() {
        let p = params(vec![], 0);
        let decoded: ChainInit = rlp::decode(&rlp::encode(&p)).unwrap();
        assert_eq!(decoded, p);
    }

    /// A length this build does not know is a genesis it cannot agree about. Guessing would mean
    /// two nodes computing different hashes from the same bytes.
    #[test]
    fn an_unknown_item_count_is_refused() {
        let mut odd = RlpStream::new();
        odd.begin_list(9);
        for _ in 0..9 {
            odd.append(&1u64);
        }
        assert!(rlp::decode::<ChainInit>(&odd.out()).is_err());
    }
}

#[cfg(test)]
mod auto_release_encoding_tests {
    use super::*;
    use rlp::Rlp;

    fn params(threshold: u8, auto_release: u64) -> ChainInit {
        ChainInit {
            chain_id: 2077,
            is_testnet: true,
            tx_fee: 1000,
            ride_request_referrer_fee_bps: 200,
            ride_offer_referrer_fee_bps: 200,
            mint_authority: "0x00000000000000000000000000000000000000aa".to_string(),
            faucet_address: "0x0000000000000000000000000000000000000000".to_string(),
            faucet_allocation: 0,
            mint_cosigners: if threshold > 1 {
                vec!["0x00000000000000000000000000000000000000bb".to_string()]
            } else {
                Vec::new()
            },
            mint_threshold: threshold,
            ride_auto_release_secs: auto_release,
        }
    }

    /// Each feature is additive, and a chain using neither must still encode as it always did.
    #[test]
    fn the_three_shapes_are_eight_ten_and_eleven() {
        assert_eq!(Rlp::new(&rlp::encode(&params(0, 0))).item_count().unwrap(), 8);
        assert_eq!(Rlp::new(&rlp::encode(&params(2, 0))).item_count().unwrap(), 10);
        assert_eq!(Rlp::new(&rlp::encode(&params(0, 7200))).item_count().unwrap(), 11);
        assert_eq!(Rlp::new(&rlp::encode(&params(2, 7200))).item_count().unwrap(), 11);
    }

    #[test]
    fn every_shape_round_trips() {
        for p in [params(0, 0), params(2, 0), params(0, 7200), params(2, 7200)] {
            let decoded: ChainInit = rlp::decode(&rlp::encode(&p)).unwrap();
            assert_eq!(decoded, p);
        }
    }

    /// Auto-release without multisig still carries the mint fields at their defaults, because the
    /// item count alone has to say unambiguously which fields are present.
    #[test]
    fn auto_release_alone_still_decodes_the_mint_fields_as_defaults() {
        let decoded: ChainInit = rlp::decode(&rlp::encode(&params(0, 7200))).unwrap();
        assert!(decoded.mint_cosigners.is_empty());
        assert_eq!(decoded.mint_threshold, 0);
        assert_eq!(decoded.ride_auto_release_secs, 7200);
    }

    #[test]
    fn uses_auto_release_tracks_the_value() {
        assert!(!params(0, 0).uses_auto_release());
        assert!(params(0, 1).uses_auto_release());
    }
}
