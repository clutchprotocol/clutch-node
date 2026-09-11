use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::node::account_state::AccountState;
use crate::node::balance_effect::{BalanceEffectKind, StateUpdate};
use crate::node::database::Database;

use sha3::{Digest, Keccak256};

use crate::node::signature_keys::SignatureKeys;

use super::address::{canonical_account_address, is_valid_address};
use super::chain_init::ChainInit;

/// One approval signature over the mint approval digest, from an authority other than the one
/// submitting the transaction. `v` is 27 or 28, the stack convention everywhere.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MintCosignature {
    pub r: String,
    pub s: String,
    pub v: u64,
}

impl Encodable for MintCosignature {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(3);
        stream.append(&self.r);
        stream.append(&self.s);
        stream.append(&self.v);
    }
}

impl Decodable for MintCosignature {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(MintCosignature {
            r: rlp.val_at(0)?,
            s: rlp.val_at(1)?,
            v: rlp.val_at(2)?,
        })
    }
}

/// What a mint approver signs: this much CLT, to this account, for this off-chain payment, on this
/// chain. 64 lowercase hex characters, no `0x`.
///
/// NOT the transaction hash, and it cannot be. The transaction hash covers the Mint arguments, and
/// the cosignatures live in those arguments, so a cosigner signing the transaction hash would have
/// to sign something containing their own signature. There is no fixed point.
///
/// `nonce` and `from` are deliberately excluded so approvers need not know which authority will
/// submit, or in what order. They approve a mint, not a transaction, which is how the four-eyes
/// flow already works. Replay is closed by `credit_ref` being exactly-once in state, so an
/// approval cannot be reused for a second mint, and `chain_id` stops one chain approval working
/// on another.
pub fn mint_approval_digest_hex(chain_id: u64, to: &str, amount: u64, credit_ref: &str) -> String {
    let mut stream = RlpStream::new();
    stream.begin_list(4);
    stream.append(&chain_id);
    stream.append(&to.to_string());
    stream.append(&amount);
    stream.append(&credit_ref.to_string());
    let mut hasher = Keccak256::new();
    hasher.update(stream.out());
    hex::encode(hasher.finalize())
}

/// Exactly-once ref marker: `processed_ref_{64-hex}` in the state CF, value = tx hash.
/// Shared by Mint (credit_ref) and Burn (redemption_ref) — refs are keccak256 hashes of
/// treasury intent ids, so one namespace cannot collide across the two uses.
pub fn processed_ref_key(reference: &str) -> Vec<u8> {
    format!("processed_ref_{}", reference).into_bytes()
}

pub fn ref_is_valid(reference: &str) -> bool {
    reference.len() == 64 && reference.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

pub fn ref_already_processed(db: &Database, reference: &str) -> Result<bool, String> {
    match db.get("state", &processed_ref_key(reference)) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(e) => Err(format!("failed to read processed ref: {}", e)),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mint {
    pub to: String,
    pub amount: u64,
    pub credit_ref: String,
    /// Approval signatures from authorities other than the submitter, over
    /// `mint_approval_digest_hex`. Empty on a single-signer chain.
    ///
    /// These sit inside the Mint arguments on purpose, so the transaction hash covers them: the
    /// submitting authority envelope signature commits to this exact set, and nobody can strip,
    /// add or swap one afterwards without invalidating it.
    #[serde(default)]
    pub cosignatures: Vec<MintCosignature>,
}

impl Mint {
    /// Is this mint authorised by enough distinct members of the chain authority set?
    ///
    /// Separated from `verify_state` because it needs no database, so the security-critical half
    /// of minting can be tested directly rather than through a chain fixture.
    ///
    /// A stolen key satisfies this on a 1-of-1 chain and satisfies nothing above it. That is the
    /// entire point: the four-eyes rule in `treasury-service` is off-chain and a stolen key walks
    /// straight past it, whereas this runs in consensus.
    pub fn check_authorisation(&self, from: &str, params: &ChainInit) -> Result<(), String> {
        let authorities = params.mint_authority_set();
        let threshold = params.effective_mint_threshold();

        let submitter = canonical_account_address(from);
        if !authorities.contains(&submitter) {
            return Err(format!("Mint rejected: '{}' is not a mint authority", from));
        }

        let digest =
            mint_approval_digest_hex(params.chain_id, &self.to, self.amount, &self.credit_ref);
        let mut signers = vec![submitter];
        for (i, cosig) in self.cosignatures.iter().enumerate() {
            let recovered = SignatureKeys::recover_address(
                digest.as_bytes(),
                &cosig.r,
                &cosig.s,
                cosig.v as i32,
            )
            .map_err(|e| format!("Mint rejected: cosignature {} does not recover: {}", i, e))?;
            let canon = canonical_account_address(&recovered);
            if !authorities.contains(&canon) {
                return Err(format!(
                    "Mint rejected: cosignature {} recovers to '{}', not a mint authority",
                    i, recovered
                ));
            }
            // One key signing twice must not count twice, or a 2-of-3 is satisfied by one key.
            if signers.contains(&canon) {
                return Err(format!(
                    "Mint rejected: cosignature {} repeats authority '{}'",
                    i, recovered
                ));
            }
            signers.push(canon);
        }
        if signers.len() < threshold {
            return Err(format!(
                "Mint rejected: {} distinct authority signature(s), needs {}",
                signers.len(),
                threshold
            ));
        }
        Ok(())
    }

    pub fn verify_state(&self, from: &String, db: &Database) -> Result<(), String> {
        let params = ChainInit::get(db)?;
        self.check_authorisation(from, &params)?;
        if !is_valid_address(&self.to) {
            return Err(format!(
                "Mint rejected: 'to' must be a 20-byte-hex address, got '{}'",
                self.to
            ));
        }
        if self.amount == 0 {
            return Err("Mint rejected: amount must be positive".to_string());
        }
        if self.amount > i64::MAX as u64 {
            return Err(
                "Mint rejected: amount exceeds i64::MAX (balance deltas are i64)".to_string(),
            );
        }
        // Same ceiling `add_block_to_chain` enforces, checked here against committed supply
        // so the *pool* refuses the mint. A mint that only failed at block application was a
        // poison pill: pool deletions live in the same uncommitted batch as the state writes,
        // so the tx survived the failed block, the authoring filter re-included it every
        // tick, and `author_new_block` failed forever with no eviction path. The block-level
        // check stays as the backstop for a hostile author.
        let supply = ChainInit::get_total_supply(db)?;
        if supply as u128 + self.amount as u128 > i64::MAX as u128 {
            return Err(format!(
                "Mint rejected: total_supply out of range: {} + {} exceeds i64::MAX",
                supply, self.amount
            ));
        }
        if !ref_is_valid(&self.credit_ref) {
            return Err("Mint rejected: credit_ref must be 64 lowercase hex chars".to_string());
        }
        if ref_already_processed(db, &self.credit_ref)? {
            return Err(format!(
                "Mint rejected: credit_ref '{}' already processed (exactly-once)",
                self.credit_ref
            ));
        }
        Ok(())
    }

    pub fn state_transaction(&self, tx_hash: &String, db: &Database) -> Vec<StateUpdate> {
        vec![
            AccountState::apply_balance_change(
                &self.to,
                self.amount as i64,
                BalanceEffectKind::Mint,
                None,
                db,
            ),
            StateUpdate::storage_only(
                processed_ref_key(&self.credit_ref),
                tx_hash.clone().into_bytes(),
            ),
        ]
    }
}

impl Encodable for Mint {
    fn rlp_append(&self, stream: &mut RlpStream) {
        // 3 items with no cosignatures, so a single-signer Mint encodes exactly as it did before
        // this field existed and its hash is unchanged.
        stream.begin_list(if self.cosignatures.is_empty() { 3 } else { 4 });
        stream.append(&self.to);
        stream.append(&self.amount);
        stream.append(&self.credit_ref);
        if !self.cosignatures.is_empty() {
            stream.append_list::<MintCosignature, MintCosignature>(&self.cosignatures);
        }
    }
}

impl Decodable for Mint {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() {
            return Err(DecoderError::RlpExpectedToBeList);
        }
        let has_cosignatures = match rlp.item_count()? {
            3 => false,
            4 => true,
            _ => return Err(DecoderError::RlpIncorrectListLen),
        };
        Ok(Mint {
            to: rlp.val_at(0)?,
            amount: rlp.val_at(1)?,
            credit_ref: rlp.val_at(2)?,
            cosignatures: if has_cosignatures {
                rlp.list_at(3)?
            } else {
                Vec::new()
            },
        })
    }
}

#[cfg(test)]
mod mofn_tests {
    use super::*;
    use rlp::{Encodable as _, Rlp};

    const SECRET_A: &str = "0101010101010101010101010101010101010101010101010101010101010101";
    const SECRET_B: &str = "0202020202020202020202020202020202020202020202020202020202020202";
    const SECRET_C: &str = "0303030303030303030303030303030303030303030303030303030303030303";
    const SECRET_OUTSIDER: &str =
        "0909090909090909090909090909090909090909090909090909090909090909";
    const REF: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TO: &str = "0x00000000000000000000000000000000000000ff";

    /// Sign `digest_hex` and return the signer address alongside the cosignature, so tests never
    /// hardcode an address they cannot derive by hand.
    fn cosign(secret: &str, digest_hex: &str) -> (String, MintCosignature) {
        let (r, s, v) = SignatureKeys::sign(secret, digest_hex.as_bytes());
        let addr = SignatureKeys::recover_address(digest_hex.as_bytes(), &r, &s, v)
            .expect("a freshly made signature must recover");
        (addr, MintCosignature { r, s, v: v as u64 })
    }

    fn address_of(secret: &str) -> String {
        cosign(secret, &"11".repeat(32)).0
    }

    fn params(authority: &str, cosigners: Vec<String>, threshold: u8) -> ChainInit {
        ChainInit {
            chain_id: 2077,
            is_testnet: true,
            tx_fee: 1000,
            ride_request_referrer_fee_bps: 200,
            ride_offer_referrer_fee_bps: 200,
            mint_authority: authority.to_string(),
            faucet_address: "0x0000000000000000000000000000000000000000".to_string(),
            faucet_allocation: 0,
            mint_cosigners: cosigners,
            mint_threshold: threshold,
            ride_auto_release_secs: 0,
        }
    }

    fn mint(cosignatures: Vec<MintCosignature>) -> Mint {
        Mint {
            to: TO.to_string(),
            amount: 5_000_000,
            credit_ref: REF.to_string(),
            cosignatures,
        }
    }

    fn digest_for(p: &ChainInit, m: &Mint) -> String {
        mint_approval_digest_hex(p.chain_id, &m.to, m.amount, &m.credit_ref)
    }

    // --- the approval digest -----------------------------------------------------------------

    #[test]
    fn approval_digest_is_64_lowercase_hex() {
        let d = mint_approval_digest_hex(2077, TO, 1, REF);
        assert_eq!(d.len(), 64);
        assert!(d.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    /// Every field an approver is agreeing to must change the thing they sign, or an approval for
    /// one mint authorises a different one.
    #[test]
    fn approval_digest_binds_every_field() {
        let base = mint_approval_digest_hex(2077, TO, 1, REF);
        assert_ne!(base, mint_approval_digest_hex(2078, TO, 1, REF), "chain_id");
        assert_ne!(
            base,
            mint_approval_digest_hex(2077, "0x00000000000000000000000000000000000000ee", 1, REF),
            "to"
        );
        assert_ne!(base, mint_approval_digest_hex(2077, TO, 2, REF), "amount");
        assert_ne!(
            base,
            mint_approval_digest_hex(2077, TO, 1, &"bb".repeat(32)),
            "credit_ref"
        );
    }

    // --- authorisation -----------------------------------------------------------------------

    #[test]
    fn single_signer_chain_accepts_the_authority_and_rejects_everyone_else() {
        let a = address_of(SECRET_A);
        let p = params(&a, vec![], 1);
        assert!(mint(vec![]).check_authorisation(&a, &p).is_ok());

        let outsider = address_of(SECRET_OUTSIDER);
        let err = mint(vec![]).check_authorisation(&outsider, &p).unwrap_err();
        assert!(err.contains("not a mint authority"), "{err}");
    }

    /// A threshold of 0 is what a genesis written before these fields existed decodes to, and it
    /// has to behave exactly like 1 rather than authorising with no signatures at all.
    #[test]
    fn threshold_zero_behaves_as_single_signer() {
        let a = address_of(SECRET_A);
        let p = params(&a, vec![], 0);
        assert_eq!(p.effective_mint_threshold(), 1);
        assert!(mint(vec![]).check_authorisation(&a, &p).is_ok());
        assert!(mint(vec![])
            .check_authorisation(&address_of(SECRET_OUTSIDER), &p)
            .is_err());
    }

    /// The whole point of the change: on a 2-of-3 chain, one key is not enough. This is the case
    /// that a stolen mint key hits.
    #[test]
    fn two_of_three_refuses_a_lone_signature() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 2);
        let err = mint(vec![]).check_authorisation(&a, &p).unwrap_err();
        assert!(
            err.contains("1 distinct authority signature(s), needs 2"),
            "{err}"
        );
    }

    #[test]
    fn two_of_three_accepts_submitter_plus_one_cosigner() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 2);
        let m0 = mint(vec![]);
        let (_, cosig) = cosign(SECRET_B, &digest_for(&p, &m0));
        assert!(mint(vec![cosig]).check_authorisation(&a, &p).is_ok());
    }

    /// Any two of the three, not just the one that happens to sit in `mint_authority`.
    #[test]
    fn any_two_members_suffice() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b.clone(), c.clone()], 2);
        let m0 = mint(vec![]);
        let (_, from_c) = cosign(SECRET_C, &digest_for(&p, &m0));
        assert!(
            mint(vec![from_c]).check_authorisation(&b, &p).is_ok(),
            "B submitting with C cosigning is a valid 2-of-3"
        );
    }

    #[test]
    fn a_cosignature_from_outside_the_set_does_not_count() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 2);
        let m0 = mint(vec![]);
        let (_, outsider) = cosign(SECRET_OUTSIDER, &digest_for(&p, &m0));
        let err = mint(vec![outsider]).check_authorisation(&a, &p).unwrap_err();
        assert!(err.contains("not a mint authority"), "{err}");
    }

    /// Without this, one stolen key satisfies a 2-of-3 by signing twice.
    #[test]
    fn one_key_cannot_sign_twice() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 2);
        let m0 = mint(vec![]);
        let (_, self_cosig) = cosign(SECRET_A, &digest_for(&p, &m0));
        let err = mint(vec![self_cosig]).check_authorisation(&a, &p).unwrap_err();
        assert!(err.contains("repeats authority"), "{err}");
    }

    #[test]
    fn two_cosignatures_from_the_same_authority_are_refused() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 3);
        let m0 = mint(vec![]);
        let (_, first) = cosign(SECRET_B, &digest_for(&p, &m0));
        let (_, second) = cosign(SECRET_B, &digest_for(&p, &m0));
        let err = mint(vec![first, second])
            .check_authorisation(&a, &p)
            .unwrap_err();
        assert!(err.contains("repeats authority"), "{err}");
    }

    /// An approval signed for a different amount must not authorise this one. This is the check
    /// that makes the digest meaningful rather than decorative.
    #[test]
    fn a_cosignature_over_a_different_mint_does_not_authorise_this_one() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 2);

        // B approves 5,000,000 — the submitter then tries to mint ten times that.
        let approved = mint(vec![]);
        let (_, cosig) = cosign(SECRET_B, &digest_for(&p, &approved));
        let mut inflated = mint(vec![cosig]);
        inflated.amount = 50_000_000;

        let err = inflated.check_authorisation(&a, &p).unwrap_err();
        assert!(
            err.contains("not a mint authority") || err.contains("does not recover"),
            "an approval for another amount must not carry over, got: {err}"
        );
    }

    #[test]
    fn a_malformed_cosignature_is_rejected_not_ignored() {
        let (a, b, c) = (
            address_of(SECRET_A),
            address_of(SECRET_B),
            address_of(SECRET_C),
        );
        let p = params(&a, vec![b, c], 2);
        let junk = MintCosignature {
            r: "00".repeat(32),
            s: "00".repeat(32),
            v: 27,
        };
        assert!(
            mint(vec![junk]).check_authorisation(&a, &p).is_err(),
            "a signature that cannot recover must fail rather than silently not count"
        );
    }

    // --- encoding ----------------------------------------------------------------------------

    /// A mint with no cosignatures must encode exactly as it did before the field existed, or
    /// every historical Mint hash changes and the chain cannot re-verify its own history.
    #[test]
    fn single_signer_mint_still_encodes_as_three_items() {
        let encoded = rlp::encode(&mint(vec![]));
        let rlp = Rlp::new(&encoded);
        assert_eq!(rlp.item_count().unwrap(), 3);
    }

    #[test]
    fn mint_round_trips_with_and_without_cosignatures() {
        let plain = mint(vec![]);
        let decoded: Mint = rlp::decode(&rlp::encode(&plain)).unwrap();
        assert_eq!(decoded.to, plain.to);
        assert!(decoded.cosignatures.is_empty());

        let (_, cosig) = cosign(SECRET_B, &"cc".repeat(32));
        let signed = mint(vec![cosig.clone()]);
        let encoded = rlp::encode(&signed);
        assert_eq!(Rlp::new(&encoded).item_count().unwrap(), 4);
        let decoded: Mint = rlp::decode(&encoded).unwrap();
        assert_eq!(decoded.cosignatures, vec![cosig]);
    }
}
