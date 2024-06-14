use clap::{ArgAction, Parser, Subcommand};
use eyre::Result;
use plain_contract::PlainContract;
use walkdir::WalkDir;

mod db;
mod functions;
mod plain_contract;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Preprocess the contracts with the given options
    PreProcess {
        /// Path to the root directory of plain contracts. The folder is expected to
        /// contain contracts stored alongside `metadata.json`.
        ///
        /// Example https://huggingface.co/datasets/Zellic/smart-contract-fiesta/tree/main/organized_contracts
        #[arg(long)]
        plain_contracts_root: Option<String>,

        /// Optionally ignore errors during processing (default: false)
        #[arg(long, action = ArgAction::SetTrue, default_value_t = false)]
        ignore_errors: bool,

        /// Optionally duckdb path, if not provided will try to read from environment variable DUCKDB_PATH
        #[arg(long)]
        duckdb_path: Option<String>,
    },
    IndexFunctions {
        /// Optionally duckdb path, if not provided will try to read from environment variable DUCKDB_PATH
        #[arg(long)]
        duckdb_path: Option<String>,
    },
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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::PreProcess {
            plain_contracts_root,
            ignore_errors,
            duckdb_path,
        } => {
            let duckdb_path = match duckdb_path {
                Some(path) => path.clone(),
                None => std::env::var("DUCKDB_PATH")
                    .unwrap_or_else(|_| panic!("DUCKDB_PATH environment variable is not set")),
            };
            let storage = db::Storage::new(&duckdb_path)?;

            if let Some(plain_contracts_root) = plain_contracts_root {
                let contracts = process_plain_contracts(plain_contracts_root, *ignore_errors).await;
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
                println!("Finished processing plain contracts: {}", contracts.len());
            }
            Ok(())
        }
    }
}
