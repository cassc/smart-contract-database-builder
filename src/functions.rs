use std::fmt::LowerHex;

use alloy_json_abi::Function;
use serde::{Deserialize, Serialize};
use tokio::signal;

use crate::utils::simple_hash;

#[derive(Debug, Serialize, Deserialize)]
pub struct ContractFunction {
    pub id: String,
    /// The contract id. A compilation output can have multiple contracts, in this
    /// case there could be multiple `contract_name`s associated with the same
    /// `contract_id`.
    pub contract_id: String,
    pub contract_name: String,
    pub function_name: String,
    pub filename: String,
    pub signature: String,
    pub selector: String,
    pub source_code: String,
}

impl ContractFunction {
    pub fn from_abi(
        contract_id: String,
        filename: String,
        contract_name: String,
        f: &Function,
        source_code: String,
    ) -> Self {
        let selector = f.selector();
        let selector = format!("0x{:04x}", selector);
        let signature = f.signature();
        let id = simple_hash(&format!("{}{}{}", contract_id, filename, selector));
        let function_name = f.name.clone();
        Self {
            id,
            contract_id,
            contract_name,
            function_name,
            filename,
            signature,
            selector,
            source_code,
        }
    }
}
