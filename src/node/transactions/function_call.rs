use serde::{Deserialize, Serialize};
use std::fmt;

use super::burn::Burn;
use super::chain_init::ChainInit;
use super::mint::Mint;
use super::{
    ride_acceptance::RideAcceptance, ride_cancel::RideCancel, ride_offer::RideOffer,
    ride_pay::RidePay, ride_request::RideRequest, ride_request_cancel::RideRequestCancel,
    transfer::Transfer,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "function_call_type", content = "arguments")]
pub enum FunctionCall {
    Transfer(Transfer),
    RideRequest(RideRequest),
    RideOffer(RideOffer),
    RideAcceptance(RideAcceptance),
    RidePay(RidePay),
    RideCancel(RideCancel),
    Mint(Mint),
    Burn(Burn),
    RideRequestCancel(RideRequestCancel),
    ChainInit(ChainInit),
}

impl fmt::Display for FunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FunctionCall::Transfer(args) => write!(f, "Transfer: {:?}", args),
            FunctionCall::RideRequest(args) => write!(f, "RideRequest: {:?}", args),
            FunctionCall::RideOffer(args) => write!(f, "RideOffer: {:?}", args),
            FunctionCall::RideAcceptance(args) => write!(f, "RideAcceptance: {:?}", args),
            FunctionCall::RidePay(args) => write!(f, "RidePay: {:?}", args),
            FunctionCall::RideCancel(args) => write!(f, "RideCancel: {:?}", args),
            FunctionCall::Mint(args) => write!(f, "Mint: {:?}", args),
            FunctionCall::Burn(args) => write!(f, "Burn: {:?}", args),
            FunctionCall::RideRequestCancel(args) => write!(f, "RideRequestCancel: {:?}", args),
            FunctionCall::ChainInit(args) => write!(f, "ChainInit: {:?}", args),
        }
    }
}
