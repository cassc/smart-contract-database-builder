

# Source dataset

https://huggingface.co/datasets/Zellic/smart-contract-fiesta contains 149,386
unique verified smart contracts, the uniqueness is determined by the hash of the
runtime bytecode of each deployed contract.

# To compile using the bundled duckdb

``` bash
cargo install --release -F duckdb-bundled
```
