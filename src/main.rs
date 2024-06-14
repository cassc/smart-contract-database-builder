use clap::{ArgAction, Parser};
use eyre::Result;
use plain_contract::PlainContract;
use walkdir::WalkDir;

mod db;
mod plain_contract;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the root directory of plain contracts
    #[arg(long)]
    plain_contracts_root: Option<String>,

    /// Optionally ignore errors during processing (default: false)
    #[arg(long, action = ArgAction::SetTrue, default_value_t = false)]
    ignore_errors: bool,

    /// Optionally duckdb path, if not provided will try to read from environment variable DUCKDB_PATH
    #[arg(long)]
    duckdb_path: Option<String>,
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

    let plain_contracts_root = cli.plain_contracts_root;
    let ignore_errors = cli.ignore_errors;
    let duckdb_path = cli
        .duckdb_path
        .unwrap_or_else(|| std::env::var("DUCKDB_PATH").expect("DUCKDB_PATH not set"));

    let storage = db::Storage::new(&duckdb_path)?;

    if let Some(plain_contracts_root) = plain_contracts_root {
        let contracts = process_plain_contracts(&plain_contracts_root, ignore_errors).await;
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
