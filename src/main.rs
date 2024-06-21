use clap::{ArgAction, Parser, Subcommand};
use db::{row_to_contract, Storage};
use eyre::Result;
use futures::future::try_join_all;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use log::{error, info};
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
    plain_contracts_root: Option<String>,

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

#[derive(Subcommand)]
enum Commands {
    /// Preprocess the contracts with the given options
    PreProcess(PreProcessArgs),
    IndexFunctions(IndexFunctionsArgs),
    DownloadSolc,
}

/// Load all plain contracts from the folder recursively
pub async fn process_plain_contracts(root: &str, ignore_errors: bool) -> Vec<PlainContract> {
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

async fn preprocess_contracts(storage: &mut Storage, args: &PreProcessArgs) -> Result<()> {
    let PreProcessArgs {
        plain_contracts_root,
        ignore_errors,
        chunk_size,
    } = args;
    if let Some(plain_contracts_root) = plain_contracts_root {
        let mut contracts = process_plain_contracts(plain_contracts_root, *ignore_errors).await;

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

        // contracts.iter().for_each(|c| {
        //     let id = c.hash();
        //     pb.inc(1);
        //     match storage.get_contract(&id) {
        //         Ok(None) => {
        //             storage.store_contract(c, Some(id)).unwrap();
        //         }
        //         Err(err) => {
        //             if !ignore_errors {
        //                 panic!("Check contract existence got error: {}", err)
        //             }
        //         }
        //         _ => {}
        //     }
        // });

        storage.disable_checkpoint()?;
        contracts.chunks_mut(*chunk_size).for_each(|chunk| {
            pb.inc(*chunk_size as u64);
            let contracts = chunk.to_vec();
            storage
                .store_contracts(contracts)
                .expect("Failed to store contracts");
        });

        storage.conn.execute("PRAGMA checkpoint", [])?;

        pb.finish();

        info!("Finished processing plain contracts: {}", contracts.len());
    }
    Ok(())
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

    storage.conn.execute("PRAGMA checkpoint", [])?;

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
    }
}

#[cfg(test)]
mod tests {
    use self::db::Storage;

    use super::*;
    use crate::plain_contract::ContractSourceType;

    const TEST_DUCKDB_PATH: &str = "/home/garfield/tmp/contracts.duckdb";

    async fn compile_and_extract_function(contract: &mut PlainContract) -> Result<()> {
        info!("Compiling contract: {}", contract.id());
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
        let mut storage = Storage::new(TEST_DUCKDB_PATH).unwrap();
        compile_standard_json(&mut storage).await?;
        compile_single_source_file(&mut storage).await?;
        compile_multi_source_files(&mut storage).await
    }
}
