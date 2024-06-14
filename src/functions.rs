use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ContractFunction {
    pub id: String,
    pub contract_id: String,
    pub filename: String,
    pub signature: String,
    pub selector: String,
    pub source_code: String,
}
