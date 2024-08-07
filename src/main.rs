use clap::{ArgAction, Parser, Subcommand};
use db::{row_to_contract, Storage};
use eyre::Result;
use futures::future::try_join_all;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use log::{debug, error, info};
use plain_contract::PlainContract;
use std::{fmt::Write, sync::Arc};
use tokio::{sync::Mutex, task};
use utils::download_all_solc_versions;
use walkdir::WalkDir;

use crate::plain_contract::ContractSource;

mod db;
mod functions;
mod plain_contract;
mod utils;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Optionally duckdb path, if not provided will try to read from environment variable DUCKDB_PATH
    #[arg(long)]
    duckdb_path: Option<String>,
}

#[derive(Parser)]
struct PreProcessArgs {
    /// Path to the root directory of plain contracts. The folder is expected to
    /// contain contracts stored alongside `metadata.json`.
    ///
    /// Example https://huggingface.co/datasets/Zellic/smart-contract-fiesta/tree/main/organized_contracts
    #[arg(long)]
    metadata_contracts_root: Option<String>,

    /// Folder containing etherscan contracts. Each contract contains a json file
    /// which contains both the metadata and the source code
    #[arg(long)]
    etherscan_contracts_root: Option<String>,

    /// Optionally ignore errors during processing (default: false)
    #[arg(long, action = ArgAction::SetTrue, default_value_t = false)]
    ignore_errors: bool,

    /// Chunk size, for faster importing contracts
    #[arg(long)]
    chunk_size: usize,
}

#[derive(Parser)]
struct IndexFunctionsArgs {
    /// How many contracts to process in one go
    #[arg(long)]
    chunk_size: usize,
}

#[derive(Parser)]
struct DownloadSolcArgs {
    /// Root folder for storing solc binaries
    #[arg(long)]
    solc_folder: Option<String>,
}

#[derive(Parser)]
struct ExportSourceArgs {
    /// The contract id to export
    #[arg(long)]
    contract_id: String,
    /// Output folder to store the source code
    #[arg(long)]
    output_folder: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Preprocess the contracts with the given options
    PreProcess(PreProcessArgs),
    /// Compile all contracts and store populate the `function` table
    IndexFunctions(IndexFunctionsArgs),
    /// Download all solc binaries
    DownloadSolc,
    /// Export source code of a contract
    ExportSource(ExportSourceArgs),
}

/// Search for all folders containing `metadata.json` and process them
pub async fn process_metadata_contracts(root: &str, ignore_errors: bool) -> Vec<PlainContract> {
    let mut contracts = Vec::with_capacity(12800);
    for entry in WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir())
    {
        let dir_path = entry.path();
        let metadata_path = dir_path.join("metadata.json");

        if metadata_path.exists() {
            match PlainContract::from_folder(&dir_path.to_string_lossy()).await {
                Ok(c) => {
                    contracts.push(c);
                }
                Err(error) => {
                    if !ignore_errors {
                        panic!("Process file failed with error {error}")
                    }
                }
            }
        }
    }
    contracts
}

/// Search and process etherscan json files and process
pub async fn process_etherscan_contracts(root: &str, ignore_errors: bool) -> Vec<PlainContract> {
    let mut contracts = Vec::with_capacity(12800);
    for entry in WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            let folder = {
                match e.path().parent() {
                    None => return false,
                    Some(parent) => match parent.file_name() {
                        None => return false,
                        Some(name) => name.to_string_lossy(),
                    },
                }
            };
            let filename = e.file_name().to_string_lossy();

            filename.starts_with(&*folder)
                && e.file_type().is_file()
                && e.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with(".json")
        })
    {
        let path = entry.path();
        match PlainContract::from_etherscan_json(&path.to_string_lossy()).await {
            Ok(c) => {
                contracts.push(c);
            }
            Err(error) => {
                if ignore_errors {
                    debug!("Process file failed with error {error} {path:?}")
                } else {
                    panic!("Process file failed with error {error} {path:?}")
                }
            }
        }
    }

    contracts
}

async fn export_source(storage: &mut Storage, args: &ExportSourceArgs) -> Result<()> {
    let contract = storage
        .get_contract(&args.contract_id)?
        .expect("Contract not found");

    contract.export_source_code(&args.output_folder).await
}

async fn preprocess_contracts(storage: &mut Storage, args: &PreProcessArgs) -> Result<()> {
    let PreProcessArgs {
        metadata_contracts_root,
        etherscan_contracts_root,
        ignore_errors,
        chunk_size,
    } = args;
    match (metadata_contracts_root, etherscan_contracts_root) {
        (None, None) => {
            panic!("At least one of the metadata_contracts_root or etherscan_contracts_root should be provided")
        }
        (Some(metadata_contracts_root), None) => {
            let mut contracts =
                process_metadata_contracts(metadata_contracts_root, *ignore_errors).await;

            info!("Total contracts: {}", contracts.len());

            let total_countracts = contracts.len();
            let pb = ProgressBar::new(total_countracts as u64);

            pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap()
            .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
                write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
            })
            .progress_chars("#>-"),
        );

            storage.disable_checkpoint()?;
            contracts.chunks_mut(*chunk_size).for_each(|chunk| {
                pb.inc(*chunk_size as u64);
                let contracts = chunk.to_vec();
                storage
                    .store_contracts(contracts)
                    .expect("Failed to store contracts");
            });

            storage.enable_checkpoint()?;

            pb.finish();

            info!("Finished processing plain contracts: {}", contracts.len());
            Ok(())
        }
        (None, Some(etherscan_contracts_root)) => {
            let mut contracts =
                process_etherscan_contracts(etherscan_contracts_root, *ignore_errors).await;

            info!("Total contracts: {}", contracts.len());

            let total_countracts = contracts.len();
            let pb = ProgressBar::new(total_countracts as u64);

            pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap()
            .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
                write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
            })
            .progress_chars("#>-"),
            );

            storage.disable_checkpoint()?;
            contracts.chunks_mut(*chunk_size).for_each(|chunk| {
                pb.inc(*chunk_size as u64);
                let contracts = chunk.to_vec();
                storage
                    .store_contracts(contracts)
                    .expect("Failed to store contracts");
            });

            storage.enable_checkpoint()?;

            pb.finish();

            info!("Finished processing plain contracts: {}", contracts.len());

            Ok(())
        }
        _ => {
            panic!("Only one of metadata_contracts_root or etherscan_contracts_root should be provided")
        }
    }
}

async fn index_functions(storage: &mut Storage, args: &IndexFunctionsArgs) -> Result<()> {
    let total_countracts = storage.count_contracts()? as u64;
    let pb = ProgressBar::new(total_countracts);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )?
            .progress_chars("#>-"),
    );

    let mut i: u64 = 0;
    let size = args.chunk_size as u64;
    loop {
        if i >= total_countracts {
            break;
        }
        let query = format!(
            "SELECT source, source_type::varchar, metadata FROM contract offset ? limit {size}"
        );
        let mut stmt = storage.conn.prepare(&query)?;
        let mut rows = stmt.query([i])?;

        let mut contracts = Vec::new();

        // Collect all contracts
        while let Some(row) = rows.next()? {
            let contract = row_to_contract(row)?;
            contracts.push(contract);
        }

        let functions = Arc::new(Mutex::new(Vec::new()));

        let compile_futures: Vec<_> = contracts
            .into_iter()
            .map(|mut contract| {
                let functions = functions.clone();
                task::spawn(async move {
                    if matches!(contract.source, ContractSource::Vyper(_)) {
                        return;
                    }
                    if let Err(e) = contract.compile().await {
                        error!("Failed to compile contract with id {} {}", contract.id(), e);
                        return;
                    }

                    match contract.extract_functions() {
                        Err(e) => {
                            log::error!(
                                "Failed to extract functions from contract with id {} {}",
                                contract.id(),
                                e
                            );
                            panic!("Failed to extract functions from contract");
                        }
                        Ok(funcs) => {
                            let mut functions = functions.lock().await;
                            functions.extend(funcs);
                        }
                    }
                })
            })
            .collect();

        try_join_all(compile_futures).await?;

        i += size;

        let functions = functions.lock().await;
        storage.store_functions(&functions)?;
        pb.inc(size);
    }

    storage.enable_checkpoint()?;

    pb.finish();

    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    let duckdb_path = match cli.duckdb_path {
        Some(path) => path.clone(),
        None => std::env::var("DUCKDB_PATH")
            .unwrap_or_else(|_| panic!("DUCKDB_PATH environment variable is not set")),
    };
    let mut storage = db::Storage::new(&duckdb_path)?;

    match &cli.command {
        Commands::IndexFunctions(args) => index_functions(&mut storage, args).await,
        Commands::PreProcess(args) => preprocess_contracts(&mut storage, args).await,
        Commands::DownloadSolc => download_all_solc_versions().await,
        Commands::ExportSource(args) => export_source(&mut storage, args).await,
    }
}

#[cfg(test)]
mod tests {
    use self::db::Storage;

    use super::*;
    use crate::plain_contract::ContractSourceType;

    async fn compile_and_extract_function(contract: &mut PlainContract) -> Result<()> {
        println!("Compiling contract: {}", contract.id());
        let output = contract.compile().await?.succeeded();
        output.assert_success();
        assert!(output.artifacts().count() > 0);

        let functions = contract.extract_functions()?;
        assert!(!functions.is_empty());

        Ok(())
    }

    async fn compile_standard_json(storage: &mut Storage) -> Result<()> {
        let mut contract = storage
            .get_random_contract(&ContractSourceType::Json, None)?
            .expect("No contract found");

        // let mut contract = storage
        //     .get_contract("499b5eda3c676626f2fd72ab579e0f88")?
        //     .expect("No contract found");

        compile_and_extract_function(&mut contract).await
    }

    async fn compile_single_source_file(storage: &mut Storage) -> Result<()> {
        let mut contract = storage
            .get_random_contract(&ContractSourceType::SingleSolidity, None)?
            .expect("No contract found");

        compile_and_extract_function(&mut contract).await
    }

    async fn compile_multi_source_files(storage: &mut Storage) -> Result<()> {
        let mut contract = storage
            .get_random_contract(&ContractSourceType::MultiSolidity, None)?
            .expect("No contract found");
        compile_and_extract_function(&mut contract).await
    }

    #[allow(dead_code)]
    async fn compile_yul_source_code(storage: &mut Storage) -> Result<()> {
        let mut contract = storage
            .get_random_contract(&ContractSourceType::MultiSolidity, None)?
            .expect("No contract found");

        compile_and_extract_function(&mut contract).await
    }

    #[tokio::test]
    async fn test_compile_and_extract_functions() -> Result<()> {
        let duckdb_path = std::env::var("TEST_DUCKDB_PATH").expect("Test db is required");
        let mut storage = Storage::new(&duckdb_path).unwrap();
        compile_standard_json(&mut storage).await?;
        compile_single_source_file(&mut storage).await?;
        compile_multi_source_files(&mut storage).await
    }

    #[tokio::test]
    async fn get_source_code_by_function_complex() -> Result<()> {
        let duckdb_path = std::env::var("TEST_DUCKDB_PATH").expect("Test db is required");
        let contract_id = "1e889892cd854c8a85230ff7bd5a2935";
        let storage = Storage::new(&duckdb_path)?;
        let mut contract = storage
            .get_contract(contract_id)?
            .expect("Contract not found");
        contract.compile().await?;

        let source = contract.source_code_by_contract_and_function_name(
            "TransparentUpgradeableProxy",
            "upgradeTo",
        )?;

        println!("{source}");

        Ok(())
    }
}
