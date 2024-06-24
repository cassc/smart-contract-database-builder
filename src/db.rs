use std::fs::create_dir_all;

use crate::{
    functions::ContractFunction,
    plain_contract::{ContractSource, ContractSourceType, Metadata, PlainContract},
};
use duckdb::{params, types::FromSql, Connection};
use eyre::Result;
use rand::Rng;

pub struct Storage {
    pub conn: Connection,
}

enum SourceType {
    SingleSolidity,
    MultiSolidity,
    Vyper,
    Json,
}

impl FromSql for SourceType {
    fn column_result(value: duckdb::types::ValueRef<'_>) -> duckdb::types::FromSqlResult<Self> {
        let s = String::column_result(value)?;
        match s.as_str() {
            "single_sol" => Ok(SourceType::SingleSolidity),
            "multi_sol" => Ok(SourceType::MultiSolidity),
            "vyper" => Ok(SourceType::Vyper),
            "json" => Ok(SourceType::Json),
            _ => unreachable!(),
        }
    }
}

pub fn row_to_contract(row: &duckdb::Row) -> Result<PlainContract> {
    let source: String = row.get(0)?;
    let source_type: SourceType = row.get(1)?;
    let metadata: String = row.get(2)?;

    let source: ContractSource = match source_type {
        SourceType::SingleSolidity => serde_json::from_str(&source)?,
        SourceType::MultiSolidity => serde_json::from_str(&source)?,
        SourceType::Vyper => serde_json::from_str(&source)?,
        SourceType::Json => serde_json::from_str(&source)?,
    };

    let metadata: Metadata = serde_json::from_str(&metadata)?;
    Ok(PlainContract::new(metadata, source))
}

impl Storage {
    pub fn new(db_file: &str) -> Result<Storage> {
        let parent = std::path::Path::new(db_file).parent();
        if let Some(parent) = parent {
            create_dir_all(parent)?;
        }

        let conn = Connection::open(db_file)?;
        let _ = conn.execute_batch(
            r"
-- Create ENUM type for source_type
CREATE TYPE source_type_enum AS ENUM ('json', 'vyper', 'single_sol', 'multi_sol');

-- Create contract table
CREATE TABLE contract (
    id STRING PRIMARY KEY,
    name STRING,
    metadata STRING,
    source STRING,
    source_type source_type_enum
);

-- Create function table with foreign key
CREATE TABLE function (
    id STRING PRIMARY KEY,
    contract_id STRING,
    contract_name STRING,
    function_name STRING,
    filename STRING,
    signature STRING,
    selector STRING,
    source_code STRING,
    FOREIGN KEY (contract_id) REFERENCES contract(id)
);

CREATE INDEX idx_function_composite ON function(contract_id, selector, signature);
",
        );

        Ok(Storage { conn })
    }

    /// Disables checkpoint on shutdown
    pub fn disable_checkpoint(&self) -> Result<()> {
        self.conn
            .execute("PRAGMA disable_checkpoint_on_shutdown;", [])?;
        Ok(())
    }

    /// Get contract by id
    #[allow(dead_code)]
    pub fn get_contract(&self, id: &str) -> Result<Option<PlainContract>> {
        let mut stmt = self.conn.prepare(
            "SELECT source, source_type::varchar, metadata FROM contract WHERE id = ? limit 1",
        )?;
        let mut rows = stmt.query([id])?;
        let row = match rows.next()? {
            Some(row) => row,
            None => return Ok(None),
        };

        Ok(Some(row_to_contract(row)?))
    }

    #[allow(dead_code)]
    pub fn get_random_contract(
        &self,
        source_type: &ContractSourceType,
        offset: Option<u32>,
    ) -> Result<Option<PlainContract>> {
        let mut stmt = self.conn.prepare(
            "SELECT source, source_type::varchar, metadata FROM contract where source_type::varchar=? OFFSET ? LIMIT 1",
        )?;
        let source_type: String = source_type.to_string();
        let mut rows = stmt.query(params![
            &source_type,
            offset.unwrap_or_else(|| {
                let mut rng = rand::thread_rng();
                rng.gen_range(0..1000)
            })
        ])?;
        let row = match rows.next()? {
            Some(row) => row,
            None => return Err(eyre::eyre!("No contract found")),
        };

        Ok(Some(row_to_contract(row)?))
    }

    /// Store a single contract
    #[allow(dead_code)]
    pub fn store_contract(&self, contract: &PlainContract, id: Option<String>) -> Result<()> {
        let PlainContract {
            metadata, source, ..
        } = contract;
        let id = id.unwrap_or_else(|| contract.hash());
        let name = &metadata.contract_name.clone();
        let source_type = match source {
            ContractSource::SingleSolidity(_) => "single_sol",
            ContractSource::MultiSolidity(_) => "multi_sol",
            ContractSource::Vyper(_) => "vyper",
            ContractSource::Json(_) => "json",
        };
        let source = serde_json::to_string(source)?;
        let metadata = serde_json::to_string(metadata)?;
        self.conn.execute(
            "INSERT INTO contract (id, name, metadata, source, source_type) VALUES (?, ?, ?, ?, ?)",
            [id, name.into(), metadata, source, source_type.into()],
        )?;

        Ok(())
    }

    /// Store multiple contracts in batch mode
    pub fn store_contracts(&self, contracts: Vec<PlainContract>) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO contract (id, name, metadata, source, source_type) VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )?;

        for c in contracts {
            let PlainContract {
                metadata, source, ..
            } = &c;
            let id: String = c.hash();
            let name: String = metadata.contract_name.clone();
            let source_type = match &source {
                ContractSource::SingleSolidity(_) => "single_sol",
                ContractSource::MultiSolidity(_) => "multi_sol",
                ContractSource::Vyper(_) => "vyper",
                ContractSource::Json(_) => "json",
            };
            let source = serde_json::to_string(&source)?;
            let metadata = serde_json::to_string(&metadata)?;
            // allow error
            let _ = stmt.insert([id, name, metadata, source, source_type.into()]);
        }

        Ok(())
    }

    pub fn count_contracts(&self) -> Result<u32> {
        let mut stmt = self.conn.prepare("SELECT COUNT(*) FROM contract")?;
        let mut rows = stmt.query([])?;
        let row = rows.next()?.unwrap();
        let count: u32 = row.get(0)?;
        Ok(count)
    }

    pub fn store_functions(&self, functions: &[ContractFunction]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR IGNORE INTO function (id, contract_id, contract_name, function_name, filename, signature, selector, source_code) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )?;

        for f in functions.iter() {
            let id = f.id.clone();
            let contract_id = f.contract_id.clone();
            let contract_name = f.contract_name.clone();
            let function_name = f.function_name.clone();
            let filename = f.filename.clone();
            let signature = f.signature.clone();
            let selector = f.selector.clone();
            let source_code = f.source_code.clone();
            // allow error
            let _ = stmt.insert([
                id,
                contract_id,
                contract_name,
                function_name,
                filename,
                signature,
                selector,
                source_code,
            ]);
        }

        Ok(())
    }
}
