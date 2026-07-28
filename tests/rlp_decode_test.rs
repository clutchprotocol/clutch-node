#[cfg(test)]
mod tests {
    use clutch_node::node::transactions::function_call::FunctionCall;
    use clutch_node::node::transactions::ride_request::RideRequest;
    use hex;
    use rlp::{Encodable, RlpStream};
    use sha3::{Digest, Keccak256};
    use clutch_node::node::{coordinate, rlp_encoding};
    use clutch_node::node::transactions::transaction::Transaction;
    use std::str::from_utf8;
    const PASSENGER_ADDRESS_KEY: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
    const PASSENGER_SECRET_KEY: &str ="d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";

    #[test]
    fn decode_rlp_to_transaction_struct() {
        // Build an 8-item fixture the same way `sdk_style_tx` in transaction.rs does: a 4-item
        // preimage `[from (no 0x), nonce, chain_id, data]`, Keccak-256 that, then the 8-item
        // wire list `[from, nonce, chain_id, signature_r, signature_s, signature_v, hash, data]`
        // with chain_id at index 2. This is the current wire contract (Task 4); the old 7-item
        // fixture predated chain_id and could only ever decode to Err.
        let from_clean = "deb4cfb63db134698e1879ea24904df074726cc0";
        let nonce: u64 = 1;
        let chain_id: u64 = 2077;

        let function_call = FunctionCall::Transfer(
            clutch_node::node::transactions::transfer::Transfer {
                to: "0x8f19077627cde4848b090c53c83b12956837d5e9".to_string(),
                value: 10,
            },
        );
        let mut data_stream = RlpStream::new();
        function_call.rlp_append(&mut data_stream);
        let data_rlp = data_stream.out();

        let mut unsigned = RlpStream::new_list(4);
        unsigned.append(&from_clean.to_string());
        unsigned.append(&nonce);
        unsigned.append(&chain_id);
        unsigned.append_raw(data_rlp.as_ref(), 1);
        let mut hasher = Keccak256::new();
        hasher.update(unsigned.out().as_ref());
        let hash_hex = hex::encode(hasher.finalize());

        let dummy = "cd".repeat(32);
        let mut full = RlpStream::new_list(8);
        full.append(&from_clean.to_string());
        full.append(&nonce);
        full.append(&chain_id);
        full.append(&dummy);
        full.append(&dummy);
        full.append(&27u64);
        full.append(&hash_hex);
        full.append_raw(data_rlp.as_ref(), 1);
        let rlp_bytes = full.out().to_vec();

        // Debug print: show each RLP field
        let rlp = rlp::Rlp::new(&rlp_bytes);
        println!("RLP item count: {}", rlp.item_count().unwrap_or(0));

        // Enhanced debugging to understand the structure better
        println!("Top level is list: {}", rlp.is_list());

        // Investigate each field to find any RLP structure issues
        for i in 0..rlp.item_count().unwrap_or(0) {
            let val = rlp.at(i).unwrap();

            // Get the bytes directly
            if let Ok(data) = val.data() {
                if let Ok(str_val) = from_utf8(data) {
                    println!("Field {}: String({:?}), bytes: {}", i, str_val, hex::encode(data));
                } else {
                    println!("Field {}: Binary, bytes: {}", i, hex::encode(data));
                }
            } else if val.is_list() {
                println!("Field {}: List with {} items", i, val.item_count().unwrap_or(0));

                // If this is field 7 (data field), print more details
                if i == 7 {
                    println!("  Data field structure:");
                    // Check if it follows the expected structure [tag, args]
                    if val.item_count().unwrap_or(0) >= 2 {
                        if let Ok(tag) = val.at(0).unwrap().as_val::<u8>() {
                            println!("  Tag: {}", tag);
                        }

                        let args = val.at(1).unwrap();
                        if args.is_list() {
                            println!("  Args is a list with {} items", args.item_count().unwrap_or(0));
                        } else {
                            println!("  Args is not a list");
                        }
                    }
                }
            } else {
                println!("Field {}: Unknown type", i);
            }
        }

        // Decode to Transaction struct and assert on the current 8-item, chain_id-bearing contract.
        let tx = rlp_encoding::decode::<Transaction>(&rlp_bytes).unwrap_or_else(|e| {
            panic!(
                "Failed to decode RLP to Transaction: {:?}. Expected RLP structure: \
                 8 items [from, nonce, chain_id, signature_r, signature_s, signature_v, hash, data] \
                 with chain_id at index 2 and 'data' a list [tag, args].",
                e
            )
        });
        println!("Decoded Transaction: {:#?}", tx);
        assert_eq!(tx.chain_id, chain_id, "chain_id must round-trip through decode");
        assert_eq!(tx.from, format!("0x{}", from_clean), "from must round-trip through decode");
        assert_eq!(tx.nonce, nonce, "nonce must round-trip through decode");
    }

    
#[test]
fn test_rlp_encode_ride_request_transaction() {
    // Create a sample RideRequest transaction and print its RLP encoding
    let ride_request = RideRequest {
        pickup_location: coordinate::Coordinates {
            latitude: 27.223374842000805,
            longitude: 56.365535283043855,
        },
        dropoff_location: coordinate::Coordinates {
            latitude: 27.225817157860583,
            longitude: 56.40913096554422,
        },
        fare: 1000,
        referrer: None,
    };
    // Use nonce 1 for example
    let mut tx = Transaction::new_transaction(
        PASSENGER_ADDRESS_KEY.to_string(),
        1,
        2077,
        FunctionCall::RideRequest(ride_request),
    );
    // Sign with passenger's secret key
    tx.sign(PASSENGER_SECRET_KEY);
    // Encode to RLP
    let encoded = clutch_node::node::rlp_encoding::encode(&tx);
    println!("RideRequest Tx RLP: 0x{}", hex::encode(&encoded));
    
    // Also print the decoded version to verify structure
    println!("\nVerifying by decoding our own encoding:");
    match rlp_encoding::decode::<Transaction>(&encoded) {
        Ok(decoded_tx) => println!("Successfully decoded our own transaction: {:?}", decoded_tx),
        Err(e) => println!("Failed to decode our own transaction: {:?}", e),
    }
}
} 