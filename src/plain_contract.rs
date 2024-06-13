use eyre::Result;
use serde::{Deserialize, Serialize};

/// Metadata of a contract
#[derive(Debug, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(rename = "ContractName")]
    pub contract_name: String,
    #[serde(rename = "CompilerVersion")]
    pub compiler_version: String,
    #[serde(rename = "Runs")]
    pub runs: u32,
    #[serde(rename = "OptimizationUsed")]
    pub optimization_used: bool,
    #[serde(rename = "BytecodeHash")]
    pub bytecode_hash: String,
}

/// A single source file
#[derive(Debug, Serialize, Deserialize)]
pub struct SourceFile {
    pub name: String,
    pub content: String,
}

/// The complete source code of a contract
#[derive(Debug, Serialize, Deserialize)]
pub enum ContractSource {
    SingleSolidity(SourceFile),
    MultiSolidity(Vec<SourceFile>),
    Viper(SourceFile),
    Json(SourceFile),
}

/// A contract with metadata and source code
#[derive(Debug, Serialize, Deserialize)]
pub struct PlainContract {
    pub metadata: Metadata,
    pub source: ContractSource,
}

fn source_from_multi_source_contract(path: &str) -> Result<ContractSource> {
    // list all solidity files in the folder
    let sources = std::fs::read_dir(path)?
        .filter_map(|entry| {
            let entry = entry.as_ref().unwrap();
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "sol") {
                Some(SourceFile {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    content: std::fs::read_to_string(path).unwrap(),
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    Ok(ContractSource::MultiSolidity(sources))
}

impl PlainContract {
    /// Parse a contract from a folder path
    pub fn from_folder(path: &str) -> Result<Self> {
        let metadata = std::fs::read_to_string(format!("{}/metadata.json", path))?;
        let metadata: Metadata = serde_json::from_str(&metadata)?;

        // There are 4 types of contracts:
        // 1. A single solidity file: main.sol
        // 2. A single viper file: main.vy
        // 3. A single json file: contract.json
        // 4. A multi-source contract containing multiple solidity files
        let contract_json = std::fs::read_to_string(format!("{}/contract.json", path));
        let solidity_source = std::fs::read_to_string(format!("{}/main.sol", path));
        let viper_source = std::fs::read_to_string(format!("{}/main.sol", path));
        match (contract_json, solidity_source, viper_source) {
            (Ok(contract_json), _, _) => {
                let name = "contract.json".into();
                let content = contract_json;
                let source = ContractSource::Json(SourceFile { name, content });
                Ok(Self { metadata, source })
            }
            (_, Ok(solidity_source), _) => {
                let name = "main.sol".into();
                let content = solidity_source;
                let source = ContractSource::SingleSolidity(SourceFile { name, content });
                Ok(Self { metadata, source })
            }
            (_, _, Ok(viper_source)) => {
                let name = "main.vy".into();
                let content = viper_source;
                let source = ContractSource::Viper(SourceFile { name, content });
                Ok(Self { metadata, source })
            }
            _ => Ok(Self {
                metadata,
                source: source_from_multi_source_contract(path)?,
            }),
        }
    }
}
