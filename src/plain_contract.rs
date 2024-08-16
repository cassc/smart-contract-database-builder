use alloy_json_abi::Function;
use duckdb::ToSql;
use eyre::{ContextCompat, Result};
use foundry_compilers::{
    artifacts::{Node, NodeType::*, Settings},
    multi::{MultiCompiler, MultiCompilerSettings},
    solc::{Solc, SolcCompiler},
    Project, ProjectCompileOutput, ProjectPathsConfig,
};

use foundry_compilers_artifacts::NodeType;
use itertools::Itertools;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt::Display,
    path::{Component, Path, PathBuf},
};
use tokio::fs::{self, create_dir_all};
use walkdir::WalkDir;

use crate::{functions::ContractFunction, utils::simple_hash};

/// Metadata of a contract
#[derive(Debug, Serialize, Deserialize, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EtherscanRawJson {
    #[serde(rename = "SourceCode")]
    pub source_code: String,
    #[serde(rename = "OptimizationUsed")]
    pub optimization_used: String,
    #[serde(rename = "Runs")]
    pub runs: String,
    #[serde(rename = "ContractName")]
    pub contract_name: String,
    #[serde(rename = "CompilerVersion")]
    pub compiler_version: String,
}

impl EtherscanRawJson {
    pub fn to_metadata(&self) -> Metadata {
        Metadata {
            contract_name: self.contract_name.clone(),
            compiler_version: self.compiler_version.clone(),
            runs: self.runs.parse().unwrap_or(0),
            optimization_used: self.optimization_used == "1",
            bytecode_hash: "".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceCodeEntry {
    pub content: String,
}

/// Standard json input file
#[derive(Debug, Serialize, Deserialize)]
pub struct StandardJson {
    pub langauge: Option<String>,
    pub name: Option<String>,
    pub sources: HashMap<String, SourceCodeEntry>,
    pub settings: Option<Settings>,
}

/// A single source file
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SourceFile {
    pub name: String,
    pub content: String,
}

/// The complete source code of a contract
#[derive(Debug, Serialize, Deserialize, Clone)]
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

impl Display for ContractSourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContractSourceType::SingleSolidity => write!(f, "single_sol"),
            ContractSourceType::MultiSolidity => write!(f, "multi_sol"),
            ContractSourceType::Vyper => write!(f, "vyper"),
            ContractSourceType::Json => write!(f, "json"),
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
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlainContract {
    pub metadata: Metadata,
    pub source: ContractSource,
    #[serde(skip)]
    pub compilation_output: Option<ProjectCompileOutput>,
    #[serde(skip)]
    pub source_files: Option<Vec<SourceFile>>,
}

async fn source_from_multi_source_contract(path: &str) -> Result<ContractSource> {
    let walker = WalkDir::new(path).into_iter();
    let mut sources = Vec::new();

    for entry in walker {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_file() && path.extension().map_or(false, |ext| ext == "sol") {
                    let content = fs::read_to_string(path).await?;
                    sources.push(SourceFile {
                        name: path.display().to_string(),
                        content,
                    });
                }
            }
            Err(e) => eprintln!("Error reading directory entry: {}", e),
        }
    }
    Ok(ContractSource::MultiSolidity(sources))
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

    fn get_source_files(&self) -> Result<Vec<SourceFile>> {
        match self {
            ContractSource::SingleSolidity(source) => Ok(vec![source.clone()]),
            ContractSource::MultiSolidity(sources) => Ok(sources.clone()),
            ContractSource::Vyper(source) => Ok(vec![source.clone()]),
            ContractSource::Json(source) => {
                let json: StandardJson = serde_json::from_str(&source.content)?;

                let sources: Vec<SourceFile> = json
                    .sources
                    .iter()
                    .map(|(name, content)| SourceFile {
                        name: name.clone(),
                        content: content.content.clone(),
                    })
                    .collect();
                Ok(sources)
            }
        }
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

    pub fn id(&self) -> String {
        self.hash()
    }

    /// Parser a contract from etherscan json
    pub async fn from_etherscan_json(path: &str) -> Result<Self> {
        let name = "contract.json".into();
        let content = fs::read_to_string(path).await?;
        let outer_json: EtherscanRawJson = serde_json::from_str(&content)?;
        let metadata = outer_json.to_metadata();
        let source_code = &outer_json.source_code;
        let source_code = if source_code.starts_with("{{") {
            let len = source_code.len();
            &source_code[1..len - 1]
        } else {
            &source_code[..]
        };

        match serde_json::from_str::<StandardJson>(source_code) {
            Ok(_std_json) => {
                let source = ContractSource::Json(SourceFile {
                    name,
                    content: source_code.into(),
                });
                Ok(Self::new(metadata, source))
            }
            Err(_) => Ok(Self::new(
                metadata,
                ContractSource::SingleSolidity(SourceFile {
                    name: "main.sol".into(),
                    content: source_code.into(),
                }),
            )),
        }
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
        let viper_source = fs::read_to_string(format!("{}/main.vy", path)).await;
        match (contract_json, solidity_source, viper_source) {
            (Ok(contract_json), _, _) => {
                let name = "contract.json".into();
                let content = contract_json;
                let source = ContractSource::Json(SourceFile { name, content });
                Ok(Self::new(metadata, source))
            }
            (_, Ok(solidity_source), _) => {
                let name = "main.sol".into();
                let content = solidity_source;
                let source = ContractSource::SingleSolidity(SourceFile { name, content });
                Ok(Self::new(metadata, source))
            }
            (_, _, Ok(viper_source)) => {
                let name = "main.vy".into();
                let content = viper_source;
                let source = ContractSource::Vyper(SourceFile { name, content });
                Ok(Self::new(metadata, source))
            }
            _ => Ok(Self::new(
                metadata,
                source_from_multi_source_contract(path).await?,
            )),
        }
    }

    pub fn get_source_files(&self) -> Result<Vec<SourceFile>> {
        self.source.get_source_files()
    }

    /// Compile the contract
    pub async fn compile(&mut self) -> Result<ProjectCompileOutput> {
        let root = tempfile::tempdir()?;
        let root_path = root.path();
        let source_path = root_path.join(&self.metadata.contract_name);

        let source_files = self.get_source_files()?;

        let v = self.metadata.compiler_version.clone();
        let v = v.trim_start_matches('v');
        let version = Version::parse(v)?;
        let version = Version::new(version.major, version.minor, version.patch);
        let solc = Solc::find_or_install(&version)?;
        let solc = SolcCompiler::Specific(solc);
        let compiler = MultiCompiler::new(solc, None)?;

        let mut settings = Settings::default();

        // TODO json is parsed twice, also parsed in writting source files for ether json
        if let ContractSource::Json(ref source) = self.source {
            let json: StandardJson = serde_json::from_str(&source.content)?;
            settings = json.settings.context("Missing settings in json")?;

            for remapping in settings.remappings.iter_mut() {
                let new_path = source_path.join(remapping.path.trim_start_matches('/'));
                remapping.path = new_path.display().to_string();
            }
        }

        ContractSource::write_entries(&source_path, &source_files.iter().collect()).await?;

        let paths = ProjectPathsConfig::builder()
            .sources(source_path.clone())
            .remappings(settings.remappings)
            .build_with_root(source_path.clone());

        let mut settings = MultiCompilerSettings::default();
        let solc_settings = settings.solc.clone().with_ast();
        settings.solc = solc_settings;
        let builder = Project::builder()
            .paths(paths)
            .ephemeral()
            .no_artifacts()
            .settings(settings);
        let builder = builder.build(compiler)?;
        let output = builder.compile()?.with_stripped_file_prefixes(&source_path);

        self.source_files = Some(source_files);
        self.compilation_output = Some(output.clone());

        if output.has_compiler_errors() {
            Err(eyre::eyre!(format!(
                "Compilation failed: {:?}",
                &output.output().errors
            )))?
        } else {
            Ok(output)
        }
    }

    pub fn new(metadata: Metadata, source: ContractSource) -> PlainContract {
        PlainContract {
            metadata,
            source,
            compilation_output: None,
            source_files: None,
        }
    }

    /// Search the function source code by contract and function name from the AST
    pub fn source_code_by_contract_and_function_name(
        &self,
        contract_name: &str,
        function_name: &str,
    ) -> Result<String> {
        let compilation_output = self
            .compilation_output
            .as_ref()
            .context("No compilation output, did you forget to call compile()?")?;

        // Contract by artifact
        let contract = compilation_output
            .artifacts()
            .find(|(name, _)| name == contract_name)
            .context("Contract not found")?;

        // Find the source file of the contract
        let (filename, _, _artifact) = compilation_output
            .artifacts_with_files()
            .find(|(_, name, _)| *name == contract_name)
            .context("Artifact not found")?;

        let source_file = contract.1.source_file();

        // AST nodes in the source file
        let mut nodes_in_source: Vec<&Node> = source_file
            .as_ref()
            .and_then(|f| f.ast.as_ref())
            .map(|ast| ast.nodes.iter())
            .unwrap_or_default()
            .collect();

        // The complete source code as text in the file
        let content = &self
            .source_files
            .as_ref()
            .context("No source files in PlainContract")?
            .iter()
            .find(|f| f.name == filename.display().to_string())
            .context("No source file matches the expected file name")?
            .content;

        // Normalize text, need this because foundry-compile does this before
        // compilation, without it offset will be wrong
        // Ref: crates/artifacts/solc/src/sources.rs
        let content = content.replace("\r\n", "\n");

        let mut nodes_in_contract = vec![];

        while nodes_in_source.len() > 1 {
            let node = nodes_in_source.pop().context("No node")?;
            match node.node_type {
                ContractDefinition
                    if node.attribute::<String>("name") == Some(contract_name.into()) =>
                {
                    nodes_in_contract.extend(&node.nodes);
                    break;
                }
                _ => {
                    let children = &node.nodes;
                    nodes_in_source.extend(children);
                }
            }
        }

        // NOTE:
        // 1. this does not find the function from parent contract
        // 2. function from public field couldn't be found
        while nodes_in_contract.len() > 1 {
            let node = nodes_in_contract.pop().context("No node")?;
            match node.node_type {
                FunctionDefinition => match node.attribute::<String>("name") {
                    Some(name) if name == function_name => {
                        let src = &node.src;
                        let start = src.start;
                        let _fid = src.index.expect("No file index in source location");
                        let length = src.length.expect("No length in source location");
                        let bytes = &content.as_bytes();
                        let source_code = &bytes[start..start + length];
                        let source_code = String::from_utf8_lossy(source_code);
                        return Ok(source_code.into());
                    }
                    _ => {}
                },
                _ => {
                    let children = &node.nodes;
                    nodes_in_contract.extend(children);
                }
            }
        }

        Err(eyre::eyre!("Function not found"))
    }

    /// Return a list of functions from the contract AST. Ignoring any interface fucntions, ie. functions without a body
    pub fn extract_functions(&self) -> Result<Vec<ContractFunction>> {
        let compilation_output = self
            .compilation_output
            .as_ref()
            .context("No compilation output")?;
        let contract_id = self.id();
        let mut functions = vec![];
        struct NodesToVisit {
            nodes: Vec<Node>,
            contract_id: String,
            file: String,
            contract_name: String,
            abi_functions: Vec<Function>,
        }
        let mut nodes_to_visit = vec![];

        compilation_output
            .artifacts_with_files()
            .for_each(|(file, contract_name, artifact)| {
                if let Some(source_file) = artifact.source_file() {
                    let abi_functions = artifact
                        .abi
                        .as_ref()
                        .map(|abi| abi.functions().cloned())
                        .unwrap_or_default()
                        .collect::<Vec<_>>();
                    if let Some(ast) = source_file.ast {
                        nodes_to_visit.push(NodesToVisit {
                            nodes: ast.nodes,
                            contract_id: contract_id.clone(),
                            file: file.display().to_string(),
                            contract_name: contract_name.clone(),
                            abi_functions,
                        });
                    }
                }
            });

        while nodes_to_visit.len() > 1 {
            let NodesToVisit {
                nodes,
                contract_id,
                file,
                contract_name,
                abi_functions,
            } = nodes_to_visit.pop().context("No node")?;
            let content = &self
                .source_files
                .as_ref()
                .context("No source files in PlainContract")?
                .iter()
                .find(|f| f.name.trim_start_matches("/") == file)
                .context(format!(
                    "No source file matches the expected file name: {}",
                    file
                ))?
                .content;
            let content = content.replace("\r\n", "\n");
            for node in nodes {
                println!("visisting {:?}", node.attribute::<String>("name"));
                match node.node_type {
                    NodeType::FunctionDefinition => {
                        let src = &node.src;
                        let start = src.start;
                        let length = src.length.expect("No length in source location");
                        let bytes = &content.as_bytes();
                        let source_code = &bytes[start..start + length];
                        let source_code = String::from_utf8_lossy(source_code);

                        // ignore interface functions
                        if source_code.trim_end().ends_with(';') {
                            continue;
                        }

                        let function_name = node.attribute::<String>("name").unwrap_or_default();

                        // TODO: it's possible to match the function with the abi and get the correct function selector
                        // let abi_func = abi_functions.iter().find(|f| f.signature == function_name);

                        let f = ContractFunction {
                            id: simple_hash(&format!(
                                "{}{}{}",
                                contract_id, function_name, source_code
                            )),
                            contract_id: contract_id.clone(),
                            contract_name: contract_name.clone(),
                            function_name,
                            source_code: source_code.into(),
                            filename: file.clone(),
                            selector: "".into(),
                            signature: "".into(),
                        };
                        functions.push(f);
                    }
                    _ => {
                        let children = node.nodes;
                        nodes_to_visit.push(NodesToVisit {
                            nodes: children,
                            contract_id: contract_id.clone(),
                            file: file.clone(),
                            contract_name: contract_name.clone(),
                            abi_functions: abi_functions.clone(),
                        });
                    }
                }
            }
        }

        Ok(functions)
    }

    /// Export source code to the output folder
    pub async fn export_source_code(&self, output_folder: &str) -> Result<()> {
        let root_path = PathBuf::from(output_folder);
        let source_path = root_path.join(&self.metadata.contract_name);

        let metadata = &self.metadata;
        let metadata_source = SourceFile {
            name: "metadata.json".into(),
            content: serde_json::to_string_pretty(metadata)?,
        };
        let to_write = vec![&metadata_source];

        ContractSource::write_entries(&source_path, &to_write).await?;

        let source_files = self.get_source_files()?;

        ContractSource::write_entries(&source_path, &source_files.iter().collect()).await
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_compile_and_get_source_by_function() -> Result<()> {
        let mut contract = PlainContract::from_folder("./contracts/demo").await?;

        let output = contract.compile().await?;
        let artificat = output
            .artifacts()
            .find(|(name, _)| name == "AdvancedCounter");

        let _ast = artificat
            .expect("No AdvancedCounter contract")
            .1
            .source_file()
            .unwrap()
            .ast;

        let source = contract.source_code_by_contract_and_function_name("Counter", "decrement");

        let expected_found =
            "function decrement() public override {\n        count = count.subtract(1);\n    }";

        assert!(matches!(source, Ok(found) if found == expected_found));

        let source =
            contract.source_code_by_contract_and_function_name("AdvancedCounter", "decrement");

        assert!(matches!(source, Err(_e)));

        let source = contract.source_code_by_contract_and_function_name("Counter", "count");

        assert!(matches!(source, Err(_e)));

        let source = contract
            .source_code_by_contract_and_function_name("AdvancedCounter", "privateFunction");

        let expected_found = "function privateFunction () private returns (uint) {\n        return block.timestamp;\n    }";

        assert!(matches!(source, Ok(found) if found == expected_found));
        Ok(())
    }

    #[tokio::test]
    async fn test_parse_etherscan_contract() -> Result<()> {
        let mut contract = PlainContract::from_etherscan_json(
            "./contracts/0x9ca84eacf0d0775782ab5b34d01187b37f1ceea4_Bueno721Drop.json",
        )
        .await?;
        contract.compile().await?;
        let functions = contract.extract_functions()?;
        for f in functions {
            println!("{}", f.filename);
            println!("{}", f.contract_name);
            println!("{}", f.contract_id);
            println!("{}", f.function_name);
            println!("{}", f.source_code);
            println!("{}", "-".repeat(80));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_parse_multisource_contract() -> Result<()> {
        let mut contract = PlainContract::from_folder("./contracts/RBPill").await?;
        contract.compile().await?;
        let source_type = &contract.source;
        println!("{:?}", source_type);

        let functions = contract.extract_functions()?;
        assert!(!functions.is_empty(), "No functions found");
        for f in functions {
            println!("{}", f.filename);
            println!("{}", f.contract_name);
            println!("{}", f.contract_id);
            println!("{}", f.function_name);
            println!("{}", f.source_code);
            println!("{}", "-".repeat(80));
        }
        Ok(())
    }
}
