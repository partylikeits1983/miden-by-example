## Miden Tx 

Miden Counter Contract Example

1) Deploy the counter contract:
```
cargo run --release --bin deploy
```

2) Increment the count in the counter contract:
```
cargo run --release --bin increment
```

3) Deploy contract that copys the count from the counter contract
```
cargo run --release --bin fpi_example
```