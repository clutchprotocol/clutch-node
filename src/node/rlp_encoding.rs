extern crate rlp;

use crate::node::transactions::transaction::Transaction;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use hex;

use super::blocks::block::Block;
use super::blocks::block_bodies::BlockBodies;
use super::blocks::block_headers::{BlockHeader, BlockHeaders};
use super::p2p_server::get_block_bodies::GetBlockBodies;
use super::p2p_server::get_block_header::GetBlockHeaders;
use super::p2p_server::handshake::Handshake;
use super::transactions::complain_arrival::ComplainArrival;
use super::transactions::confirm_arrival::ConfirmArrival;
use super::transactions::function_call::FunctionCall;
use super::transactions::ride_acceptance::RideAcceptance;
use super::transactions::ride_cancel::RideCancel;
use super::transactions::ride_offer::RideOffer;
use super::transactions::ride_pay::RidePay;
use super::transactions::ride_request::RideRequest;
use super::transactions::ride_request_cancel::RideRequestCancel;
use super::transactions::transfer::Transfer;

impl Encodable for FunctionCall {
    fn rlp_append(&self, stream: &mut RlpStream) {
        match self {
            FunctionCall::Transfer(args) => {
                stream.begin_list(2);
                stream.append(&0u8); // Tag for Transfer
                stream.append(args);
            }
            FunctionCall::RideRequest(args) => {
                stream.begin_list(2);
                stream.append(&1u8); // Tag for RideRequest
                stream.append(args);
            }
            FunctionCall::RideOffer(args) => {
                stream.begin_list(2);
                stream.append(&2u8); // Tag for RideOffer
                stream.append(args);
            }
            FunctionCall::RideAcceptance(args) => {
                stream.begin_list(2);
                stream.append(&3u8); // Tag for RideAcceptance
                stream.append(args);
            }
            FunctionCall::RidePay(args) => {
                stream.begin_list(2);
                stream.append(&4u8); // Tag for RidePay
                stream.append(args);
            }
            FunctionCall::RideCancel(args) => {
                stream.begin_list(2);
                stream.append(&5u8); // Tag for RideCancel
                stream.append(args);
            }
            FunctionCall::RideRequestCancel(args) => {
                stream.begin_list(2);
                stream.append(&8u8); // Tag for RideRequestCancel
                stream.append(args);
            }
            FunctionCall::ConfirmArrival(args) => {
                stream.begin_list(2);
                stream.append(&6u8); // Tag for ConfirmArrival
                stream.append(args);
            }
            FunctionCall::ComplainArrival(args) => {
                stream.begin_list(2);
                stream.append(&7u8); // Tag for ComplainArrival
                stream.append(args);
            }
        }
    }
}

impl Decodable for FunctionCall {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        // Expecting a list of two items: tag and arguments
        if !rlp.is_list() || rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }

        let tag: u8 = rlp.val_at(0)?;
        match tag {
            0 => {
                let args: Transfer = rlp.val_at(1)?;
                Ok(FunctionCall::Transfer(args))
            }
            1 => {
                let args: RideRequest = rlp.val_at(1)?;
                Ok(FunctionCall::RideRequest(args))
            }
            2 => {
                let args: RideOffer = rlp.val_at(1)?;
                Ok(FunctionCall::RideOffer(args))
            }
            3 => {
                let args: RideAcceptance = rlp.val_at(1)?;
                Ok(FunctionCall::RideAcceptance(args))
            }
            4 => {
                let args: RidePay = rlp.val_at(1)?;
                Ok(FunctionCall::RidePay(args))
            }
            5 => {
                let args: RideCancel = rlp.val_at(1)?;
                Ok(FunctionCall::RideCancel(args))
            }
            8 => {
                let args: RideRequestCancel = rlp.val_at(1)?;
                Ok(FunctionCall::RideRequestCancel(args))
            }
            6 => {
                let args: ConfirmArrival = rlp.val_at(1)?;
                Ok(FunctionCall::ConfirmArrival(args))
            }
            7 => {
                let args: ComplainArrival = rlp.val_at(1)?;
                Ok(FunctionCall::ComplainArrival(args))
            }
            _ => Err(DecoderError::Custom("Unknown FunctionCall variant")),
        }
    }
}

impl Encodable for Transaction {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(7);

        stream.append(&self.from);
        stream.append(&self.nonce);
        stream.append(&self.signature_r);
        stream.append(&self.signature_s);
        let signature_v_as_u64 = self.signature_v as u64;
        stream.append(&signature_v_as_u64);
        stream.append(&self.hash);
        stream.append(&self.data);
    }
}

impl Decodable for Transaction {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }            
        
        // Handle 'from' field which may be encoded as binary data by JavaScript RLP library
        let from = {
            let from_item = rlp.at(0)?;
            let from_value = if let Ok(string_val) = from_item.as_val::<String>() {
                // Direct string decoding (from Rust-generated RLP)
                string_val
            } else if let Ok(bytes_val) = from_item.as_val::<Vec<u8>>() {
                // Binary data decoding (from JavaScript RLP library)
                hex::encode(&bytes_val)
            } else {
                return Err(DecoderError::Custom("Unable to decode 'from' field as string or bytes"));
            };
            
            // Ensure 'from' field has 0x prefix
            if from_value.starts_with("0x") {
                from_value
            } else {
                format!("0x{}", from_value)
            }
        };
        
        Ok(Transaction {
            from,
            nonce: rlp.val_at(1)?,
            signature_r: rlp.val_at(2)?,
            signature_s: rlp.val_at(3)?,
            signature_v: rlp.val_at::<u64>(4)? as i32,
            hash: rlp.val_at(5)?,
            data: rlp.val_at(6)?,
        })
    }
}

impl Encodable for Block {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(9);

        stream.append(&self.index);
        stream.append(&self.timestamp);
        stream.append(&self.previous_hash);
        stream.append(&self.author);
        stream.append(&self.signature_r);
        stream.append(&self.signature_s);
        let signature_v_as_u64 = self.signature_v as u64;
        stream.append(&signature_v_as_u64);
        stream.append(&self.hash);
        stream.append_list(&self.transactions);
    }
}

impl Decodable for Block {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 9 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(Block {
            index: rlp.val_at(0)?,
            timestamp: rlp.val_at(1)?,
            previous_hash: rlp.val_at(2)?,
            author: rlp.val_at(3)?,
            signature_r: rlp.val_at(4)?,
            signature_s: rlp.val_at(5)?,
            signature_v: rlp.val_at::<u64>(6)? as i32,
            hash: rlp.val_at(7)?,
            transactions: rlp.list_at(8)?,
        })
    }
}

impl Encodable for Handshake {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(3);
        stream.append(&self.genesis_block_hash);
        stream.append(&self.latest_block_hash);
        stream.append(&self.latest_block_index);
    }
}

impl Decodable for Handshake {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(Handshake {
            genesis_block_hash: rlp.val_at(0)?,
            latest_block_hash: rlp.val_at(1)?,
            latest_block_index: rlp.val_at(2)?,
        })
    }
}

impl Encodable for GetBlockHeaders {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(3);
        stream.append(&self.start_block_index);
        stream.append(&self.skip);
        stream.append(&self.limit);
    }
}

impl Decodable for GetBlockHeaders {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(GetBlockHeaders {
            start_block_index: rlp.val_at(0)?,
            skip: rlp.val_at(1)?,
            limit: rlp.val_at(2)?,
        })
    }
}

impl Encodable for BlockHeader {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(7);
        stream.append(&self.index);
        stream.append(&self.previous_hash);
        stream.append(&self.author);
        stream.append(&self.signature_r);
        stream.append(&self.signature_s);
        let signature_v_as_u64 = self.signature_v as u64;
        stream.append(&signature_v_as_u64);
        stream.append(&self.hash);
    }
}

impl Decodable for BlockHeader {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(BlockHeader {
            index: rlp.val_at(0)?,
            previous_hash: rlp.val_at(1)?,
            author: rlp.val_at(2)?,
            signature_r: rlp.val_at(3)?,
            signature_s: rlp.val_at(4)?,
            signature_v: rlp.val_at::<u64>(5)? as i32,
            hash: rlp.val_at(6)?,
        })
    }
}

impl Encodable for BlockHeaders {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append_list(&self.block_headers);
    }
}

impl Decodable for BlockHeaders {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 1 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(BlockHeaders {
            block_headers: rlp.list_at(0)?,
        })
    }
}

impl Encodable for GetBlockBodies {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append_list(&self.block_indexes);
    }
}

impl Decodable for GetBlockBodies {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 1 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(GetBlockBodies {
            block_indexes: rlp.list_at(0)?,
        })
    }
}

impl Encodable for BlockBodies {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(1);
        stream.append_list(&self.blocks);
    }
}

impl Decodable for BlockBodies {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if !rlp.is_list() || rlp.item_count()? != 1 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        
        Ok(BlockBodies {
            blocks: rlp.list_at(0)?,
        })
    }
}

pub fn encode<T: Encodable>(data: &T) -> Vec<u8> {
    let mut stream = RlpStream::new();
    data.rlp_append(&mut stream);
    stream.out().to_vec()
}

pub fn decode<T: Decodable>(bytes: &[u8]) -> Result<T, DecoderError> {
    let rlp = Rlp::new(bytes);
    T::decode(&rlp)
}

#[cfg(test)]
mod tests {

    use tracing::{error, info};

    use crate::node::time_utils::get_current_timespan;

    use super::*;

    #[test]
    fn test_encode_decode_transaction() {
        let function_call = FunctionCall::Transfer(Transfer {
            to: "0x8f19077627cde4848b090c53c83b12956837d5e9".to_string(),
            value: 10,
        });

        let tx = Transaction {
            from: "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
            data: function_call,
            nonce: 1,
            signature_r: "3b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c32"
                .to_string(),
            signature_s: "296086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23908"
                .to_string(),
            signature_v: 27,
            hash: "0086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2db".to_string(),
        };

        let encoded = encode(&tx);
        info!("Encoded: {:?}", encoded);

        let decoded = decode::<Transaction>(&encoded);
        match decoded {
            Ok(tx) => info!("Decoded: {:?}", tx),
            Err(e) => error!("Failed to decode transaction: {:?}", e),
        }
    }

    #[test]
    fn test_encode_decode_block() {
        let tx1 = Transaction {
            from: "0xdeb4cfb63db134698e1879ea24904df074726cc0".to_string(),
            data: FunctionCall::Transfer(Transfer {
                to: "0x8f19077627cde4848b090c53c83b12956837d5e9".to_string(),
                value: 10,
            }),
            nonce: 1,
            signature_r: "3b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c32"
                .to_string(),
            signature_s: "296086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23908"
                .to_string(),
            signature_v: 27,
            hash: "0086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2db".to_string(),
        };

        let tx2 = Transaction {
            from: "0xabc4cfb63db134698e1879ea24904df074726cc0".to_string(),          
            data: FunctionCall::Transfer(Transfer {
                to: "0x1f19077627cde4848b090c53c83b12956837d5e9".to_string(),
                value: 5,
            }),
            nonce: 2,
            signature_r: "2b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c33"
                .to_string(),
            signature_s: "396086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23909"
                .to_string(),
            signature_v: 28,
            hash: "1086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2db".to_string(),
        };

        let block = Block {
            index: 1,
            timestamp: get_current_timespan(),
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            author: "0x1234cfb63db134698e1879ea24904df074726cc0".to_string(),
            signature_r: "4b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c34"
                .to_string(),
            signature_s: "496086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23910"
                .to_string(),
            signature_v: 27,
            hash: "2086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2dc".to_string(),
            transactions: vec![tx1, tx2],
        };

        let encoded = encode(&block);
        info!("Encoded Block: {:?}", encoded);

        let decoded = decode::<Block>(&encoded);
        match decoded {
            Ok(block) => info!("Decoded Block: {:?}", block),
            Err(e) => error!("Failed to decode block: {:?}", e),
        }
    }

    #[test]
    fn test_encode_decode_get_block_headers() {
        let get_block_headers = GetBlockHeaders {
            start_block_index: 0,
            skip: 0,
            limit: 100,
        };

        let encoded = encode(&get_block_headers);
        info!("Encoded: {:?}", encoded);

        let decoded = decode::<GetBlockHeaders>(&encoded);
        match decoded {
            Ok(tx) => info!("Decoded: {:?}", tx),
            Err(e) => error!("Failed to decode transaction: {:?}", e),
        }
    }

    #[test]
    fn test_encode_decode_block_headers() {
        let block_header_1 = BlockHeader {
            index: 1,
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            author: "0x1234cfb63db134698e1879ea24904df074726cc0".to_string(),
            signature_r: "4b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c34"
                .to_string(),
            signature_s: "496086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23910"
                .to_string(),
            signature_v: 27,
            hash: "2086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2dc".to_string(),
        };

        let block_header_2 = BlockHeader {
            index: 1,
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000002"
                .to_string(),
            author: "0x1234cfb63db134698e1879ea24904df074726cc0".to_string(),
            signature_r: "4b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c34"
                .to_string(),
            signature_s: "496086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23910"
                .to_string(),
            signature_v: 27,
            hash: "2086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2dc".to_string(),
        };

        let block_headers = BlockHeaders {
            block_headers: vec![block_header_1, block_header_2],
        };

        let encoded = encode(&block_headers);
        info!("Encoded: {:?}", encoded);

        let decoded = decode::<BlockHeaders>(&encoded);
        match decoded {
            Ok(tx) => info!("Decoded: {:?}", tx),
            Err(e) => error!("Failed to decode BlockHeaders: {:?}", e),
        }
    }

    #[test]
    fn test_encode_decode_block_bodies() {
        let block = Block {
            index: 1,
            timestamp: get_current_timespan(),
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            author: "0x1234cfb63db134698e1879ea24904df074726cc0".to_string(),
            signature_r: "4b0cb46ae73d852bb75653ed1f1710676b0b736cd33aefc0c96e6e11417a4c34"
                .to_string(),
            signature_s: "496086bdc703286c0727c59e07b727cadfc2fe7b9c061149e4a86e726ed23910"
                .to_string(),
            signature_v: 27,
            hash: "2086095648e3160d0dfa5d40bdf4693d8a00d77ed3fb3b607156465b3e0de2dc".to_string(),
            transactions: vec![],
        };

        let block_boodies = BlockBodies {
            blocks: vec![block],
        };

        let encoded = encode(&block_boodies);
        info!("Encoded: {:?}", encoded);

        let decoded = decode::<BlockBodies>(&encoded);
        match decoded {
            Ok(tx) => info!("Decoded: {:?}", tx),
            Err(e) => error!("Failed to decode BlockBodies: {:?}", e),
        }
    }
}
