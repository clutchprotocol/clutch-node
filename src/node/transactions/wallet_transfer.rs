//! A CLT transfer signed by an Ethereum wallet (MetaMask, Trust Wallet) as an ordinary
//! Ethereum transaction.
//!
//! A wallet will only send a coin by signing a legacy Ethereum transaction:
//! `RLP[nonce, gasPrice, gasLimit, to, value, data, v, r, s]`, with the EIP-155 chain id folded
//! into `v`. It cannot be asked to sign a Clutch transaction. So the node takes that signed
//! transaction as it is, and keeps every field the signature covers, so that any node can rebuild
//! the exact bytes the wallet signed and check them:
//!
//! - **from** is recovered from the signature; nothing in the wallet's bytes names it.
//! - **nonce**: an Ethereum nonce counts from 0, a Clutch nonce from 1, so the Clutch nonce is the
//!   wallet's nonce plus one.
//! - **value**: a wallet counts in 18 decimals, CLT in 6, so the wallet's value is the CLT value
//!   times 10^12. A value with more than 6 decimals cannot be represented and is refused, never
//!   rounded.
//! - **gas price and gas limit** are kept only because they are signed. The fee is still the flat
//!   `tx_fee`; `gas_price * gas_limit` must be at least that fee in wei, so the wallet never shows
//!   less than it is charged.
//! - **wallet_chain_id** is the EIP-155 id. It must equal the node's configured wallet chain id,
//!   which is what stops a signature made for one network, Ethereum included, from being used on
//!   another.
//! - **hash** is the Ethereum transaction hash, Keccak-256 of the signed bytes, the same hash the
//!   wallet shows and asks for.
//!
//! The data field must be empty: there are no contracts. Only legacy (type 0) transactions with
//! EIP-155 are accepted; the Hub's RPC reports no base fee, so wallets send legacy.
//!
//! Accepting this type is a consensus change, gated by `WalletTransferRule`: a node with no rule,
//! or at a height below its `from_block`, rejects it, in the pool and in a block alike.

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use super::function_call::FunctionCall;
use super::transaction::Transaction;
use super::transfer::Transfer;
use crate::node::signature_keys::SignatureKeys;

/// Wei per CLT base unit: 18 decimals in the wallet, 6 on the chain.
pub const WEI_PER_BASE_UNIT: u128 = 1_000_000_000_000;

/// Half the secp256k1 group order. A signature with a larger `s` is the malleable twin of a
/// valid one and is refused, as Ethereum does (EIP-2), so one transfer has one hash.
const SECP256K1_HALF_N: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

/// When a node accepts wallet transfers. Local config, but a consensus rule: every validator of a
/// chain must carry the same values, or one rejects a block another authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletTransferRule {
    /// The EIP-155 chain id wallets sign with (the Hub's `wallet_chain_id`).
    pub wallet_chain_id: u64,
    /// The first block height that may carry one.
    pub from_block: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalletTransfer {
    /// `0x` + 40 lower-case hex.
    pub to: String,
    /// CLT base units (6 decimals).
    pub value: u64,
    pub gas_price: u64,
    pub gas_limit: u64,
    pub wallet_chain_id: u64,
}

impl WalletTransfer {
    pub fn as_transfer(&self) -> Transfer {
        Transfer {
            to: self.to.clone(),
            value: self.value,
        }
    }

    /// The fee this transfer authorizes, in wei.
    pub fn max_fee_wei(&self) -> u128 {
        self.gas_price as u128 * self.gas_limit as u128
    }

    fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let hex_part = self.to.strip_prefix("0x").unwrap_or(&self.to);
        let bytes = hex::decode(hex_part).map_err(|_| "wallet transfer 'to' is not hex".to_string())?;
        if bytes.len() != 20 {
            return Err("wallet transfer 'to' is not 20 bytes".to_string());
        }
        Ok(bytes)
    }

    fn append_body(&self, stream: &mut RlpStream, eth_nonce: u64) -> Result<(), String> {
        stream.append(&eth_nonce);
        stream.append(&self.gas_price);
        stream.append(&self.gas_limit);
        stream.append(&self.to_bytes()?);
        stream.append(&(self.value as u128 * WEI_PER_BASE_UNIT));
        stream.append_empty_data(); // data
        Ok(())
    }

    /// The EIP-155 bytes the wallet signed: `RLP[nonce, gasPrice, gasLimit, to, value, data,
    /// chainId, 0, 0]`. `SignatureKeys::verify` puts them through Keccak-256 itself.
    pub fn signing_bytes(&self, eth_nonce: u64) -> Result<Vec<u8>, String> {
        let mut stream = RlpStream::new_list(9);
        self.append_body(&mut stream, eth_nonce)?;
        stream.append(&self.wallet_chain_id);
        stream.append_empty_data();
        stream.append_empty_data();
        Ok(stream.out().to_vec())
    }

    /// The signed Ethereum transaction, byte for byte what the wallet sent.
    /// `recovery` is 0 or 1; `r` and `s` are 32-byte big-endian.
    pub fn signed_bytes(
        &self,
        eth_nonce: u64,
        recovery: u64,
        r: &[u8],
        s: &[u8],
    ) -> Result<Vec<u8>, String> {
        let v = self
            .wallet_chain_id
            .checked_mul(2)
            .and_then(|x| x.checked_add(35 + recovery))
            .ok_or("wallet chain id overflows v")?;
        let mut stream = RlpStream::new_list(9);
        self.append_body(&mut stream, eth_nonce)?;
        stream.append(&v);
        stream.append(&trim_leading_zeros(r));
        stream.append(&trim_leading_zeros(s));
        Ok(stream.out().to_vec())
    }

    /// The Ethereum hash of a node transaction carrying this transfer, `0x` + 64 hex.
    pub fn transaction_hash(tx: &Transaction, wt: &WalletTransfer) -> Result<String, String> {
        let (r, s, recovery) = signature_parts(tx)?;
        let eth_nonce = tx.nonce.checked_sub(1).ok_or("wallet transfer nonce must be at least 1")?;
        let bytes = wt.signed_bytes(eth_nonce, recovery, &r, &s)?;
        Ok(format!("0x{}", hex::encode(Keccak256::digest(&bytes))))
    }

    /// Checks the wallet's signature over the EIP-155 bytes against `tx.from`.
    pub fn verify_signature(tx: &Transaction, wt: &WalletTransfer) -> Result<(), String> {
        let (_, s, _) = signature_parts(tx)?;
        if s.as_slice() > SECP256K1_HALF_N.as_slice() {
            return Err("Verification failed: wallet signature has a high s value".to_string());
        }
        let eth_nonce = tx.nonce.checked_sub(1).ok_or("wallet transfer nonce must be at least 1")?;
        let bytes = wt.signing_bytes(eth_nonce)?;
        match SignatureKeys::verify(
            &tx.from,
            &bytes,
            &tx.signature_r,
            &tx.signature_s,
            tx.signature_v,
        ) {
            Ok(true) => Ok(()),
            Ok(false) => Err(
                "Verification failed: wallet signature does not match the from address".to_string(),
            ),
            Err(e) => Err(e),
        }
    }

    /// A raw signed Ethereum transaction (as `eth_sendRawTransaction` carries it) to a node
    /// transaction on the chain `chain_id`. Checks the shape and recovers the sender; the chain's
    /// rules (wallet chain id, activation, balance, nonce, fee) are `validate_transaction`'s.
    pub fn decode_raw(raw: &[u8], chain_id: u64) -> Result<Transaction, String> {
        match raw.first() {
            None => return Err("empty transaction".to_string()),
            Some(b) if *b < 0xc0 => {
                return Err(
                    "Only legacy transactions are supported. Your wallet sent a typed (EIP-2718) transaction."
                        .to_string(),
                )
            }
            _ => {}
        }
        let rlp = Rlp::new(raw);
        if rlp.item_count().map_err(rlp_err)? != 9 {
            return Err("a legacy transaction has 9 fields".to_string());
        }
        let eth_nonce: u64 = rlp.val_at(0).map_err(rlp_err)?;
        let gas_price: u64 = rlp
            .val_at(1)
            .map_err(|_| "gas price is too large".to_string())?;
        let gas_limit: u64 = rlp.val_at(2).map_err(rlp_err)?;
        let to: Vec<u8> = rlp.val_at(3).map_err(rlp_err)?;
        if to.len() != 20 {
            return Err("A transfer needs a recipient. Contract creation is not supported.".to_string());
        }
        let value_bytes: Vec<u8> = rlp.val_at(4).map_err(rlp_err)?;
        let data: Vec<u8> = rlp.val_at(5).map_err(rlp_err)?;
        if !data.is_empty() {
            return Err("This network has no contracts: a transfer carries no data.".to_string());
        }
        let v: u64 = rlp.val_at(6).map_err(rlp_err)?;
        let r: Vec<u8> = rlp.val_at(7).map_err(rlp_err)?;
        let s: Vec<u8> = rlp.val_at(8).map_err(rlp_err)?;
        if r.len() > 32 || s.len() > 32 {
            return Err("signature r and s are at most 32 bytes".to_string());
        }
        if v < 35 {
            return Err(
                "The transaction has no chain id (pre-EIP-155). Sign it for the Clutch network."
                    .to_string(),
            );
        }
        let wallet_chain_id = (v - 35) / 2;
        let recovery = (v - 35) % 2;

        if value_bytes.len() > 16 {
            return Err("value is too large".to_string());
        }
        let mut wei = 0u128;
        for b in &value_bytes {
            wei = (wei << 8) | *b as u128;
        }
        if wei % WEI_PER_BASE_UNIT != 0 {
            return Err("CLT has 6 decimals. Send an amount with at most 6 decimal places.".to_string());
        }
        let value = u64::try_from(wei / WEI_PER_BASE_UNIT).map_err(|_| "value is too large".to_string())?;
        if value == 0 {
            return Err("Send an amount above zero.".to_string());
        }

        let wt = WalletTransfer {
            to: format!("0x{}", hex::encode(&to)),
            value,
            gas_price,
            gas_limit,
            wallet_chain_id,
        };

        // Re-encode and require the same bytes, so the hash the node stores is the hash the
        // wallet computed, and a non-canonical encoding cannot give one transfer a second hash.
        let r32 = left_pad_32(&r);
        let s32 = left_pad_32(&s);
        if wt.signed_bytes(eth_nonce, recovery, &r32, &s32)? != raw {
            return Err("the transaction is not canonically encoded".to_string());
        }

        let signature_r = hex::encode(r32);
        let signature_s = hex::encode(s32);
        let signature_v = 27 + recovery as i32;
        let from = SignatureKeys::recover_address(
            &wt.signing_bytes(eth_nonce)?,
            &signature_r,
            &signature_s,
            signature_v,
        )?;

        let mut tx = Transaction {
            from,
            data: FunctionCall::WalletTransfer(wt),
            nonce: eth_nonce.checked_add(1).ok_or("nonce is too large")?,
            chain_id,
            signature_r,
            signature_s,
            signature_v,
            hash: String::new(),
        };
        tx.hash = format!("0x{}", hex::encode(Keccak256::digest(raw)));
        Ok(tx)
    }
}

fn rlp_err(e: DecoderError) -> String {
    format!("malformed transaction: {}", e)
}

fn trim_leading_zeros(bytes: &[u8]) -> Vec<u8> {
    let start = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes[start..].to_vec()
}

fn left_pad_32(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(bytes);
    out
}

/// `(r, s, recovery)` from a node transaction's signature fields.
fn signature_parts(tx: &Transaction) -> Result<([u8; 32], [u8; 32], u64), String> {
    let r = hex::decode(tx.signature_r.trim_start_matches("0x")).map_err(|_| "Invalid hex in r")?;
    let s = hex::decode(tx.signature_s.trim_start_matches("0x")).map_err(|_| "Invalid hex in s")?;
    if r.len() != 32 || s.len() != 32 {
        return Err("r and s must each be 32 bytes".to_string());
    }
    let recovery = match tx.signature_v {
        27 => 0,
        28 => 1,
        _ => return Err("Invalid recovery ID".to_string()),
    };
    Ok((left_pad_32(&r), left_pad_32(&s), recovery))
}

impl Encodable for WalletTransfer {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(5);
        stream.append(&self.to);
        stream.append(&self.value);
        stream.append(&self.gas_price);
        stream.append(&self.gas_limit);
        stream.append(&self.wallet_chain_id);
    }
}

impl Decodable for WalletTransfer {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 5 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(WalletTransfer {
            to: rlp.val_at(0)?,
            value: rlp.val_at(1)?,
            gas_price: rlp.val_at(2)?,
            gas_limit: rlp.val_at(3)?,
            wallet_chain_id: rlp.val_at(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";

    fn sample() -> WalletTransfer {
        WalletTransfer {
            to: "0x8f19077627cde4848b090c53c83b12956837d5e9".to_string(),
            value: 1_500_000,
            gas_price: 48_000_000_000,
            gas_limit: 21_000,
            wallet_chain_id: 20771,
        }
    }

    /// Signs like a wallet: EIP-155 over the signing bytes, low s.
    fn wallet_sign(wt: &WalletTransfer, eth_nonce: u64) -> Vec<u8> {
        let (r, s, v) = SignatureKeys::sign(KEY, &wt.signing_bytes(eth_nonce).unwrap());
        let r = hex::decode(r).unwrap();
        let s = hex::decode(s).unwrap();
        wt.signed_bytes(eth_nonce, (v - 27) as u64, &r, &s).unwrap()
    }

    #[test]
    fn decode_round_trips_and_recovers_the_sender() {
        let wt = sample();
        let raw = wallet_sign(&wt, 4);
        let tx = WalletTransfer::decode_raw(&raw, 2077).unwrap();
        let keys_addr = {
            let (r, s, v) = SignatureKeys::sign(KEY, b"x");
            SignatureKeys::recover_address(b"x", &r, &s, v).unwrap()
        };
        assert_eq!(tx.from, keys_addr);
        assert_eq!(tx.nonce, 5, "the Clutch nonce is the wallet's plus one");
        assert_eq!(tx.chain_id, 2077);
        assert_eq!(tx.hash, format!("0x{}", hex::encode(Keccak256::digest(&raw))));
        match &tx.data {
            FunctionCall::WalletTransfer(got) => assert_eq!(got, &wt),
            other => panic!("unexpected {:?}", other),
        }
        WalletTransfer::verify_signature(&tx, &wt).unwrap();
        assert_eq!(WalletTransfer::transaction_hash(&tx, &wt).unwrap(), tx.hash);
    }

    #[test]
    fn amounts_with_more_than_six_decimals_are_refused() {
        let wt = sample();
        let raw = wallet_sign(&wt, 0);
        // Rebuild with value + 1 wei.
        let rlp = Rlp::new(&raw);
        let mut stream = RlpStream::new_list(9);
        for i in 0..9 {
            if i == 4 {
                stream.append(&(wt.value as u128 * WEI_PER_BASE_UNIT + 1));
            } else {
                stream.append_raw(rlp.at(i).unwrap().as_raw(), 1);
            }
        }
        let err = WalletTransfer::decode_raw(&stream.out(), 2077).unwrap_err();
        assert!(err.contains("6 decimal"), "{err}");
    }

    #[test]
    fn typed_transactions_and_pre_eip155_are_refused() {
        assert!(WalletTransfer::decode_raw(&[0x02, 0xc0], 1).unwrap_err().contains("legacy"));
        let wt = sample();
        let mut stream = RlpStream::new_list(9);
        wt.append_body(&mut stream, 0).unwrap();
        stream.append(&27u64);
        stream.append(&vec![1u8; 32]);
        stream.append(&vec![1u8; 32]);
        assert!(WalletTransfer::decode_raw(&stream.out(), 1).unwrap_err().contains("EIP-155"));
    }

    #[test]
    fn data_is_refused() {
        let wt = sample();
        let mut stream = RlpStream::new_list(9);
        stream.append(&0u64);
        stream.append(&wt.gas_price);
        stream.append(&wt.gas_limit);
        stream.append(&wt.to_bytes().unwrap());
        stream.append(&(wt.value as u128 * WEI_PER_BASE_UNIT));
        stream.append(&vec![0xa9u8, 0x05]);
        stream.append(&(20771u64 * 2 + 35));
        stream.append(&vec![1u8; 32]);
        stream.append(&vec![1u8; 32]);
        assert!(WalletTransfer::decode_raw(&stream.out(), 1).unwrap_err().contains("no contracts"));
    }

    #[test]
    fn a_tampered_field_breaks_the_signature() {
        let wt = sample();
        let raw = wallet_sign(&wt, 0);
        let tx = WalletTransfer::decode_raw(&raw, 2077).unwrap();
        let mut more = wt.clone();
        more.value += 1;
        assert!(WalletTransfer::verify_signature(&tx, &more).is_err());
    }

    #[test]
    fn rlp_round_trip() {
        let wt = sample();
        let back: WalletTransfer = rlp::decode(&rlp::encode(&wt)).unwrap();
        assert_eq!(back, wt);
    }
}
