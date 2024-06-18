use duckdb::ToSql;
use eyre::{ContextCompat, Result};
use foundry_compilers::{
    artifacts::Settings,
    multi::{MultiCompiler, MultiCompilerSettings},
    solc::{Solc, SolcCompiler},
    Project, ProjectCompileOutput, ProjectPathsConfig,
};
use itertools::Itertools;
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};
use tokio::fs::{self, create_dir_all};
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceCodeEntry {
    pub content: String,
}

/// Etherscan json file
#[derive(Debug, Serialize, Deserialize)]
pub struct EtherscanJson {
    pub langauge: Option<String>,
    pub name: Option<String>,
    pub sources: HashMap<String, SourceCodeEntry>,
    pub settings: Option<Settings>,
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

/// The type of the contract source
#[derive(Debug, Serialize, Deserialize)]
pub enum ContractSourceType {
    #[serde(rename = "single_sol")]
    SingleSolidity,
    #[serde(rename = "multi_sol")]
    MultiSolidity,
    #[serde(rename = "vyper")]
    Vyper,
    #[serde(rename = "json")]
    Json,
}

impl ToString for ContractSourceType {
    fn to_string(&self) -> String {
        match self {
            ContractSourceType::SingleSolidity => "single_sol".into(),
            ContractSourceType::MultiSolidity => "multi_sol".into(),
            ContractSourceType::Vyper => "vyper".into(),
            ContractSourceType::Json => "json".into(),
        }
    }
}

impl ToSql for ContractSourceType {
    fn to_sql(&self) -> duckdb::Result<duckdb::types::ToSqlOutput<'_>> {
        Ok(duckdb::types::ToSqlOutput::Owned(
            duckdb::types::Value::Enum(match self {
                ContractSourceType::SingleSolidity => "single_sol".into(),
                ContractSourceType::MultiSolidity => "multi_sol".into(),
                ContractSourceType::Vyper => "vyper".into(),
                ContractSourceType::Json => "json".into(),
            }),
        ))
    }
}

/// A contract with metadata and source code
#[derive(Debug, Serialize, Deserialize)]
pub struct PlainContract {
    pub metadata: Metadata,
    pub source: ContractSource,
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

    /// Copy and expand the source code files to file system
    /// NOTE: copied from foundry
    pub async fn write_to(&self, dir: &Path) -> Result<()> {
        create_dir_all(dir).await?;
        let entries = match self {
            ContractSource::SingleSolidity(source) => vec![source],
            ContractSource::MultiSolidity(sources) => sources.iter().collect(),
            ContractSource::Vyper(source) => vec![source],
            ContractSource::Json(source) => vec![source],
        };
        Self::write_entries(dir, &entries).await
    }

    async fn write_entries(dir: &Path, entries: &Vec<&SourceFile>) -> Result<()> {
        create_dir_all(dir).await?;
        for entry in entries {
            let mut sanitized_path = sanitize_path(&entry.name);
            if sanitized_path.extension().is_none() {
                let with_extension = sanitized_path.with_extension("sol");
                if !entries
                    .iter()
                    .any(|e| PathBuf::from(e.name.clone()) == with_extension)
                {
                    sanitized_path = with_extension;
                }
            }
            let joined = dir.join(sanitized_path);
            if let Some(parent) = joined.parent() {
                create_dir_all(parent).await?;
                fs::write(joined, &entry.content).await?;
            }
        }
        Ok(())
    }
}

/// Remove any components in a smart contract source path that could cause a directory traversal.
pub(crate) fn sanitize_path(path: impl AsRef<Path>) -> PathBuf {
    let sanitized = path
        .as_ref()
        .components()
        .filter(|x| x.as_os_str() != Component::ParentDir.as_os_str())
        .collect::<PathBuf>();

    // Force absolute paths to be relative
    sanitized
        .strip_prefix("/")
        .map(PathBuf::from)
        .unwrap_or(sanitized)
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

    /// Compile the contract
    pub async fn compile(&self) -> Result<ProjectCompileOutput> {
        let root = tempfile::tempdir()?;
        let root_path = root.path();
        let source_path = root_path.join(&self.metadata.contract_name);
        self.source.write_to(&source_path).await?;

        let v = self.metadata.compiler_version.clone();
        let v = v.trim_start_matches('v');
        let version = Version::parse(v)?;
        let version = Version::new(version.major, version.minor, version.patch);
        let solc = Solc::install(&version).await?;
        let solc = SolcCompiler::Specific(solc);
        let compiler = MultiCompiler::new(solc, None)?;

        let mut settings = Settings::default();
        if let ContractSource::Json(ref source) = self.source {
            println!(
                "Compiling with settings from contract.json {}",
                &source.content
            );
            let json: EtherscanJson = serde_json::from_str(&source.content)?;
            settings = json.settings.context("Missing settings in json")?;

            for remapping in settings.remappings.iter_mut() {
                let new_path = source_path.join(remapping.path.trim_start_matches('/'));
                remapping.path = new_path.display().to_string();
            }

            let sources: Vec<SourceFile> = json
                .sources
                .iter()
                .map(|(name, content)| SourceFile {
                    name: name.clone(),
                    content: content.content.clone(),
                })
                .collect();
            let sources = sources.iter().collect();
            ContractSource::write_entries(&source_path, &sources).await?;
        }

        let paths = ProjectPathsConfig::builder()
            .sources(source_path)
            .remappings(settings.remappings)
            .build_with_root(root_path);

        let builder = Project::builder().paths(paths).ephemeral().no_artifacts();

        Ok(builder.build(compiler)?.compile()?)
    }
}
