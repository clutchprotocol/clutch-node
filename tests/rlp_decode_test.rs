#[cfg(test)]
mod tests {
    use clutch_node::node::transactions::function_call::FunctionCall;
    use clutch_node::node::transactions::ride_request::RideRequest;
    use hex;    
    use clutch_node::node::{coordinate, rlp_encoding};
    use clutch_node::node::transactions::transaction::Transaction;
    use std::str::from_utf8;
    const PASSENGER_ADDRESS_KEY: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
    const PASSENGER_SECRET_KEY: &str ="d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";    
   
    #[test]
    fn decode_rlp_to_transaction_struct() {
        // Example RLP-encoded transaction hex (replace with your actual test vector if needed)        
        let rlp_hex = "0xf9010994deb4cfb63db134698e1879ea24904df074726cc002b84065633261346332363133373836336564363330306361316236626666333266363063653562316530306631366661616337663063353738326536373963303166b840376132636365346234353637383865306535393933383533373361303036636263306433653135343064366264313664356561646262643638623733666230641cb84035333134653461653437656262653230663862663233356531353266363135366461636435666163616131303836656461396664633631663163356162393834eb01e9d288403b300b626d50c988404c2529f6b47e10d288403b35ac4197d81888404c2b187e7693508203e8";
        let rlp_bytes = hex::decode(rlp_hex.trim_start_matches("0x")).expect("Invalid hex");

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
                
                // If this is field 6 (data field), print more details
                if i == 6 {
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

        // Decode to Transaction struct
        match rlp_encoding::decode::<Transaction>(&rlp_bytes) {
            Ok(tx) => println!("Decoded Transaction: {:#?}", tx),
            Err(e) => {
                println!("Failed to decode RLP to Transaction: {:?}", e);
                // Print more details about expected structure
                println!("Expected RLP structure for Transaction:");
                println!("- 7 items in top-level list");
                println!("- Fields: [from, nonce, signature_r, signature_s, signature_v, hash, data]");
                println!("- 'data' should be a list [tag, args] where:");
                println!("  - tag is a u8 (0-7) indicating function call type");
                println!("  - args varies depending on tag");
            },
        }
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