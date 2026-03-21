use serde::{Deserialize, Serialize};
use std::fmt;

use super::{complain_arrival::ComplainArrival, confirm_arrival::ConfirmArrival, ride_acceptance::RideAcceptance, ride_cancel::RideCancel, ride_offer::RideOffer, ride_pay::RidePay, ride_request::RideRequest, ride_request_cancel::RideRequestCancel, transfer::Transfer};



#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "function_call_type", content = "arguments")]
pub enum FunctionCall {
    Transfer(Transfer),
    RideRequest(RideRequest),
    RideOffer(RideOffer),
    RideAcceptance(RideAcceptance),
    RidePay(RidePay),
    RideCancel(RideCancel),
    RideRequestCancel(RideRequestCancel),
    ConfirmArrival(ConfirmArrival),
    ComplainArrival(ComplainArrival),
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
            FunctionCall::RideRequestCancel(args) => write!(f, "RideRequestCancel: {:?}", args),
            FunctionCall::ConfirmArrival(args) => write!(f, "ConfirmArrival: {:?}", args),
            FunctionCall::ComplainArrival(args) => write!(f, "ComplainArrival: {:?}", args),
        }
    }
}