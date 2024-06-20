use clap::{ArgAction, Parser, Subcommand};
use db::{row_to_contract, Storage};
use eyre::Result;
use plain_contract::PlainContract;
use walkdir::WalkDir;

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

    /// Optinal chunk size, for faster importing contracts. Use this only if you have an empty database to start with
    #[arg(long)]
    chunk_size: Option<usize>,
}

#[derive(Parser)]
struct IndexFunctionsArgs {}

#[derive(Subcommand)]
enum Commands {
    /// Preprocess the contracts with the given options
    PreProcess(PreProcessArgs),
    IndexFunctions(IndexFunctionsArgs),
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
        let chunk_size = chunk_size.unwrap_or(0);
        if chunk_size <= 1 {
            contracts.iter().for_each(|c| {
                let id = c.hash();
                match storage.get_contract(&id) {
                    Ok(None) => {
                        storage.store_contract(c, Some(id)).unwrap();
                    }
                    Err(err) => {
                        if !ignore_errors {
                            panic!("Check contract existence got error: {}", err)
                        }
                    }
                    _ => {}
                }
            });
        } else {
            // TODO no data written to db?
            contracts.chunks_mut(chunk_size).for_each(|chunk| {
                let contracts = chunk.to_vec();
                storage
                    .store_contracts(contracts)
                    .expect("Failed to store contracts");
            });
        }

        println!("Finished processing plain contracts: {}", contracts.len());
    }
    Ok(())
}

async fn index_functions(storage: &mut Storage) -> Result<()> {
    let mut i = 0;
    let size = 1000;
    loop {
        let query = format!(
            "SELECT source, source_type::varchar, metadata FROM contract offset ? limit {size}"
        );
        let mut stmt = storage.conn.prepare(&query)?;
        let mut rows = stmt.query([i])?;

        let mut found = false;
        while let Some(row) = rows.next()? {
            let mut contract = row_to_contract(row)?;
            contract.compile().await?;
            let functions = contract.extract_functions()?;
            storage.store_functions(&functions)?;
            found = true;
        }

        i += size;

        if !found {
            break;
        }
    }

    Ok(())
}

#[tokio::main]
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
        Commands::IndexFunctions(_) => index_functions(&mut storage).await,
        Commands::PreProcess(args) => preprocess_contracts(&mut storage, args).await,
    }
}

#[cfg(test)]
mod tests {
    use self::db::Storage;

    use super::*;
    use crate::plain_contract::ContractSourceType;

    const TEST_DUCKDB_PATH: &str = "/home/garfield/tmp/contracts.duckdb";

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
        let mut storage = Storage::new(TEST_DUCKDB_PATH).unwrap();
        compile_standard_json(&mut storage).await?;
        compile_single_source_file(&mut storage).await?;
        compile_multi_source_files(&mut storage).await
    }
}
