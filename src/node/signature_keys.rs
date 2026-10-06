use hex::FromHex;
use rand::rngs::OsRng;
use secp256k1::{
    ecdsa::RecoverableSignature, ecdsa::RecoveryId, Message, PublicKey, Secp256k1, SecretKey,
};
use sha3::{Digest, Keccak256};

#[derive(Debug)]
#[allow(dead_code)]
pub struct SignatureKeys {
    pub secret_key: String,
    pub public_key: String,
    pub address_key: String,
}

impl SignatureKeys {

    #[allow(dead_code)]
    pub fn generate_new_keypair() -> Self {
        let secp = Secp256k1::new();
        let mut rng = OsRng::default();
        let (secret_key, public_key) = secp.generate_keypair(&mut rng);
        let address_key = Self::derive_address(&public_key);

        SignatureKeys {
            secret_key: hex::encode(secret_key.as_ref()),
            public_key: hex::encode(public_key.serialize_uncompressed()),
            address_key: address_key,
        }
    }

    fn derive_address(public_key: &PublicKey) -> String {
        let serialized_pubkey = public_key.serialize_uncompressed();
        let mut hasher = Keccak256::new();
        hasher.update(&serialized_pubkey[1..]);
        let hash = hasher.finalize();

        let address_key = format!("0x{}", hex::encode(&hash[12..32]));
        address_key
    }

    /// Strip 0x/0X prefix for hex parsing (Rust hex crate does not accept it)
    fn strip_hex_prefix(s: &str) -> &str {
        s.trim_start_matches("0x").trim_start_matches("0X")
    }

    /// The bytes a wallet's `personal_sign` (EIP-191, version `0x45`) puts through Keccak-256:
    /// a fixed prefix, the message length in decimal, then the message.
    ///
    /// `sign`, `recover_address` and `verify` all hash their input with Keccak-256, so giving them
    /// these bytes in place of the message gives a wallet's digest exactly. MetaMask and Trust
    /// Wallet will not sign a bare hash, and they will sign this.
    pub fn personal_sign_bytes(message: &[u8]) -> Vec<u8> {
        let mut bytes = format!("\x19Ethereum Signed Message:\n{}", message.len()).into_bytes();
        bytes.extend_from_slice(message);
        bytes
    }

    pub fn sign(secret_key: &str, data: &[u8]) -> (String, String, i32) {
        let secp = Secp256k1::new();

        let secret_key_bytes = hex::decode(Self::strip_hex_prefix(secret_key)).unwrap();
        let secret_key = SecretKey::from_slice(&secret_key_bytes).unwrap();

        // Create a message hash (Keccak-256 of the data)
        let mut hasher = Keccak256::new();
        hasher.update(data);
        let message_hash = hasher.finalize();

        // Create a message object for secp256k1
        let message = Message::from_digest_slice(&message_hash)
            .expect("Message could not be created from hash");

        // Sign the message
        let recoverable_sig = secp.sign_ecdsa_recoverable(&message, &secret_key);

        // Serialize the signature to compact format
        let (recid, sig) = recoverable_sig.serialize_compact();

        // Convert signature and recovery ID to appropriate formats
        let r = hex::encode(&sig[0..32]); // r component
        let s = hex::encode(&sig[32..64]); // s component
        let v = recid.to_i32() + 27; // recovery ID, adjusted for Ethereum (v = 27 or 28)

        (r, s, v)
    }

    /// Recover the signing address from a signature over `data`.
    ///
    /// `verify` answers "was it this one address", which is the wrong question when a signature
    /// may legitimately come from any member of a set — the M-of-N mint authority, where looping
    /// `verify` over the set would be N recoveries to learn what one recovery already knows.
    /// Returns the address in `0x`-prefixed lowercase hex; run it through
    /// `canonical_account_address` before comparing against a configured value.
    pub fn recover_address(data: &[u8], r: &str, s: &str, v: i32) -> Result<String, String> {
        let secp = Secp256k1::new();
        let mut hasher = Keccak256::new();
        hasher.update(data);
        let message_hash = hasher.finalize();
        let message = Message::from_digest_slice(&message_hash)
            .map_err(|_| "Message could not be created from hash".to_string())?;

        let sig_r =
            Vec::from_hex(Self::strip_hex_prefix(r)).map_err(|_| "Invalid hex in r".to_string())?;
        let sig_s =
            Vec::from_hex(Self::strip_hex_prefix(s)).map_err(|_| "Invalid hex in s".to_string())?;
        if sig_r.len() != 32 || sig_s.len() != 32 {
            return Err("r and s must each be 32 bytes".to_string());
        }
        let signature_data = [&sig_r[..], &sig_s[..]].concat();
        let recovery_id =
            RecoveryId::from_i32(v - 27).map_err(|_| "Invalid recovery ID".to_string())?;
        let recoverable_sig = RecoverableSignature::from_compact(&signature_data, recovery_id)
            .map_err(|_| "Valid signature could not be created".to_string())?;

        secp.recover_ecdsa(&message, &recoverable_sig)
            .map(|pk| Self::derive_address(&pk))
            .map_err(|_| "Public key could not be recovered".to_string())
    }

    pub fn verify(
        derive_address: &str,
        data: &[u8],
        r: &str,
        s: &str,
        v: i32,
    ) -> Result<bool, String> {
        Self::recover_address(data, r, s, v).map(|recovered| recovered == derive_address)
    }
}

#[cfg(test)]
mod tests {
    use tracing::{error, info};

    use super::*;

    #[test]
    fn test_generate_new_keypair() {
        let keys = SignatureKeys::generate_new_keypair();
        info!(
            "{:?},{:?},{:?}",
            keys.address_key, keys.secret_key, keys.public_key
        )
    }

    #[test]
    fn test_sign_and_verify() {
        let keys = SignatureKeys::generate_new_keypair();
        let data = b"Blockchain technology";
        info!("Public key: {:?}", keys.public_key);
        info!("Address: {:?}", keys.address_key);
        info!("Secret key: {:?}", keys.secret_key);

        // Test signing
        let (r, s, v) = SignatureKeys::sign(&keys.secret_key, data);
        info!("Signature: r={:?}, s={:?}, v={:?}", r, s, v);

        match SignatureKeys::verify(&keys.address_key, data, &r, &s, v) {
            Ok(is_verified) => assert!(is_verified, "Signature verification should succeed"),
            Err(e) => error!("Signature verification failed with error: {}", e),
        }
    }

    #[test]
    fn test_sign_and_verify_failure_on_modified_data() {
        let keys = SignatureKeys::generate_new_keypair();
        let original_data = b"Blockchain technology";
        let modified_data = b"Altered data";

        // Test signing with the original data
        let (r, s, v) = SignatureKeys::sign(&keys.secret_key, original_data);

        // Attempt to verify signature against modified data
        match SignatureKeys::verify(&keys.address_key, modified_data, &r, &s, v) {
            Ok(is_verified) => assert!(
                !is_verified,
                "Signature verification should fail on modified data"
            ),
            Err(_) => assert!(true, "Expected verification failure on modified data"),
        }
    }

    #[test]
    fn test_sign_and_verify_failure_on_wrong_key() {
        let keys = SignatureKeys::generate_new_keypair();
        let other_keys = SignatureKeys::generate_new_keypair(); // Generate a different key pair
        let data = b"Blockchain technology";

        // Test signing with the first key
        let (r, s, v) = SignatureKeys::sign(&keys.secret_key, data);

        // Attempt to verify signature with a different public key
        match SignatureKeys::verify(&other_keys.address_key, data, &r, &s, v) {
            Ok(is_verified) => assert!(
                !is_verified,
                "Signature verification should fail with a different public key"
            ),
            Err(_) => assert!(
                true,
                "Expected verification failure with a different public key"
            ),
        }
    }

    // A wallet signs with `personal_sign` (EIP-191), not over a bare hash.

    /// The committed dev key used across the repo, and its address.
    const DEV_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
    const DEV_ADDRESS: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";

    #[test]
    fn personal_sign_bytes_has_the_eip191_layout() {
        assert_eq!(
            SignatureKeys::personal_sign_bytes(b"hello"),
            b"\x19Ethereum Signed Message:\n5hello".to_vec()
        );
        // The length counts bytes, not characters: "é" is two bytes.
        assert_eq!(
            SignatureKeys::personal_sign_bytes("é".as_bytes()),
            "\x19Ethereum Signed Message:\n2é".as_bytes().to_vec()
        );
        // The length is written in decimal, however many digits it needs.
        let long = "x".repeat(123);
        let mut expected = b"\x19Ethereum Signed Message:\n123".to_vec();
        expected.extend_from_slice(long.as_bytes());
        assert_eq!(SignatureKeys::personal_sign_bytes(long.as_bytes()), expected);
    }

    #[test]
    fn personal_sign_digest_matches_the_published_hello_world_vector() {
        // `hashMessage("Hello World")` from the ethers documentation: it does not come from this code.
        let digest = Keccak256::digest(SignatureKeys::personal_sign_bytes(b"Hello World"));
        assert_eq!(
            hex::encode(digest),
            "a1de988600a42c4b4ab089b619297c17d53cffae5d5120d82d8a92d0bb3b78f2"
        );
    }

    #[test]
    fn a_signature_made_by_a_javascript_library_verifies() {
        // Made by @noble/secp256k1 (what the SDK uses) over personal_sign of this text, which is
        // what MetaMask and Trust Wallet do: a different implementation of the same standard.
        let text = "clutch-tx:1000:6f1e0b5d3a9c4e7f8a2b1c0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f";
        let bytes = SignatureKeys::personal_sign_bytes(text.as_bytes());
        let r = "03a910ef2c3144e635a9cd5dfb87d0f0a8e9cde11013b5fdbbd3098eb66ede07";
        let s = "7270323fea8d69ffeedac84df538c026d630e027b4380686287350fcb8e03c91";
        assert_eq!(SignatureKeys::verify(DEV_ADDRESS, &bytes, r, s, 28), Ok(true));
        // The bare text is another digest: the same signature must not verify for it.
        assert_eq!(
            SignatureKeys::verify(DEV_ADDRESS, text.as_bytes(), r, s, 28),
            Ok(false)
        );
        // And the key behind the vector is the one we think it is.
        let (r2, s2, v2) = SignatureKeys::sign(DEV_SK, &bytes);
        assert_eq!(SignatureKeys::verify(DEV_ADDRESS, &bytes, &r2, &s2, v2), Ok(true));
    }

    #[test]
    fn a_wallet_signature_binds_the_signer_and_the_exact_text() {
        let keys = SignatureKeys::generate_new_keypair();
        let other = SignatureKeys::generate_new_keypair();
        let bytes = SignatureKeys::personal_sign_bytes(b"clutch-tx:1000:abcd");
        let (r, s, v) = SignatureKeys::sign(&keys.secret_key, &bytes);

        assert_eq!(SignatureKeys::verify(&keys.address_key, &bytes, &r, &s, v), Ok(true));
        assert_eq!(SignatureKeys::verify(&other.address_key, &bytes, &r, &s, v), Ok(false));
        let other_chain = SignatureKeys::personal_sign_bytes(b"clutch-tx:1001:abcd");
        assert_eq!(
            SignatureKeys::verify(&keys.address_key, &other_chain, &r, &s, v),
            Ok(false)
        );
    }
}
