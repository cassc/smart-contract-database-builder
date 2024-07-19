

# Source dataset

Build smart contract database from the source code. [Zellic/smart-contract-fiesta](https://huggingface.co/datasets/Zellic/smart-contract-fiesta) and all Solidity contract with json format from etherscan are supported.

# To compile using the bundled duckdb

``` bash
cargo install --release -F duckdb-bundled
```

# Usage

``` bash
smart-contract-database-builder -h

Usage: smart-contract-database-builder [OPTIONS] <COMMAND>

Commands:
  pre-process      Preprocess the contracts with the given options
  index-functions  Compile all contracts and store populate the `function` table
  download-solc    Download all solc binaries
  export-source    Export source code of a contract
  help             Print this message or the help of the given subcommand(s)

Options:
      --duckdb-path <DUCKDB_PATH>  Optionally duckdb path, if not provided will try to read from environment variable DUCKDB_PATH
  -h, --help                       Print help
  -V, --version                    Print version
```

Download the solc binaries:

``` bash
smart-contract-database-builder download-solc
```

This will add all the contracts in table with name `contract` in the database `contracts.duckdb` in the current directory:


``` bash
DUCKDB_PATH=contracts.duckdb  smart-contract-database-builder pre-process --etherscan-contracts-root path-to-verfied-contracts-from-etherscan --chunk-size 100 --ignore-errors
```

This will compile all the contracts and populate the `function` table:

``` bash
DUCKDB_PATH=contracts.duckdb  smart-contract-database-builder index-functions --chunk-size 20
```
