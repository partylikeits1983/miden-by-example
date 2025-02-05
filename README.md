## Miden Tx 

Miden Counter Contract Example

1) Deploy the counter contract:
```
cargo run --release --bin deploy
```

2) Modify the `counter_contract_id` in `src/bin/increment.rs` and then increment the count in the counter contract:
```
cargo run --release --bin increment
```

3) Modify the `counter_contract_id` in `src/bin/fpi_example.rs`, then deploy contract that copys the count from the counter contract
```
cargo run --release --bin fpi_example
```