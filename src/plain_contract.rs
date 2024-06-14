use eyre::Result;
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio_stream::{wrappers::ReadDirStream, StreamExt};

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
    Vyper(SourceFile),
    Json(SourceFile),
}

/// A contract with metadata and source code
#[derive(Debug, Serialize, Deserialize)]
pub struct PlainContract {
    pub metadata: Metadata,
    pub source: ContractSource,
}

pub struct CompilationOutput {
    pub bytecode: String,
    pub abi: String,
}

async fn source_from_multi_source_contract(path: &str) -> Result<ContractSource> {
    // list all solidity files in the folder
    let folder = fs::read_dir(path).await?;
    let mut entries = ReadDirStream::new(folder);

    let mut sources = Vec::new();
    while let Some(entry) = entries.next().await {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "sol") {
                    sources.push(SourceFile {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        content: fs::read_to_string(path).await?,
                    });
                }
            }
            Err(e) => eprintln!("Error reading directory entry: {}", e),
        }
    }
    Ok(ContractSource::MultiSolidity(sources))
}


/// Hashing the content after removing all the whitespaces
fn simple_hash(content: &str) -> String {
    let re = Regex::new(r"\s+").unwrap();
    let result = re.replace_all(content, "");
    let digest = md5::compute(result.as_bytes());
    format!("{:x}", digest)
}

impl ContractSource {
    pub fn hash(&self) -> String {
        match self {
            ContractSource::SingleSolidity(source) => simple_hash(&source.content),
            ContractSource::MultiSolidity(sources) => {
                // hash each source file, sort them and join them as a single string
                let joined_hashes = sources
                    .iter()
                    .map(|source| simple_hash(&source.content))
                    .sorted()
                    .join("");
                simple_hash(&joined_hashes)
            }
            ContractSource::Vyper(source) => simple_hash(&source.content),
            ContractSource::Json(source) => simple_hash(&source.content),
        }
    }
}



impl PlainContract {
    pub fn hash(&self) -> String {
        self.source.hash()
    }

    /// Parse a contract from a folder path
    pub async fn from_folder(path: &str) -> Result<Self> {
        let metadata = fs::read_to_string(format!("{}/metadata.json", path)).await?;
        let metadata: Metadata = serde_json::from_str(&metadata)?;

        // There are 4 types of contracts:
        // 1. A single solidity file: main.sol
        // 2. A single viper file: main.vy
        // 3. A single json file: contract.json
        // 4. A multi-source contract containing multiple solidity files
        let contract_json = fs::read_to_string(format!("{}/contract.json", path)).await;
        let solidity_source = fs::read_to_string(format!("{}/main.sol", path)).await;
        let viper_source = fs::read_to_string(format!("{}/main.sol", path)).await;
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
                let source = ContractSource::Vyper(SourceFile { name, content });
                Ok(Self { metadata, source })
            }
            _ => Ok(Self {
                metadata,
                source: source_from_multi_source_contract(path).await?,
            }),
        }
    }

    pub async compile(&self) -> Result<CompilationOutput> {
        todo!()
    }
}
