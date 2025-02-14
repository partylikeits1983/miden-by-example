use std::sync::Arc;

use rand::Rng;

use miden_client::{
    account::{
        component::{BasicWallet, RpoFalcon512},
        AccountBuilder, AccountStorageMode, AccountType,
    },
    crypto::RpoRandomCoin,
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    Client, ClientError, Felt,
};

use miden_objects::crypto::dsa::rpo_falcon512::SecretKey;

/// Initialize the client (unchanged from your snippet).
pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
    // RPC endpoint and timeout
    let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let timeout_ms = 10_000;

    let rpc_api = Box::new(TonicRpcClient::new(endpoint, timeout_ms));

    let mut seed_rng = rand::thread_rng();
    let coin_seed: [u64; 4] = seed_rng.gen();
    let rng = RpoRandomCoin::new(coin_seed.map(Felt::new));

    let store_path = "store.sqlite3";
    let store = SqliteStore::new(store_path.into())
        .await
        .map_err(ClientError::StoreError)?;
    let arc_store = Arc::new(store);
    let authenticator = StoreAuthenticator::new_with_rng(arc_store.clone(), rng.clone());

    let client = Client::new(rpc_api, rng, arc_store, Arc::new(authenticator), true);
    Ok(client)
}

fn matches_pattern(address: &str, pattern: &str) -> bool {
    // First, ensure they are exactly the same length.
    if address.len() != pattern.len() {
        return false;
    }

    // Now compare character-by-character.
    for (addr_char, pat_char) in address.chars().zip(pattern.chars()) {
        // If pattern has 'X', it is a wildcard; skip checks.
        if pat_char == 'X' {
            continue;
        }
        // If pattern char is not 'X', it must match exactly.
        if addr_char != pat_char {
            return false;
        }
    }

    // If we never hit a mismatch, it matches.
    true
}

#[tokio::main]
async fn main() -> Result<(), ClientError> {
    // Pattern to look for
    let user_pattern = "0xXXXXXXXXXXXXXXXXXXXXXXXXXXbeef";

    let mut client = initialize_client().await?;

    // let anchor_block = client.get_epoch_block(123.into()).await.unwrap();
    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let key_pair = SecretKey::with_rng(client.rng());

    println!("Brute forcing addresses to match pattern: {}", user_pattern);

    loop {
        // Generate a random seed for the account creation
        let mut seed_rng = rand::thread_rng();
        let init_seed: [u8; 32] = seed_rng.gen();

        let builder = AccountBuilder::new(init_seed)
            .anchor((&anchor_block).try_into().unwrap())
            .account_type(AccountType::RegularAccountUpdatableCode)
            .storage_mode(AccountStorageMode::Public)
            .with_component(RpoFalcon512::new(key_pair.public_key()))
            .with_component(BasicWallet);

        // Build the account
        let (account, _seed) = match builder.build() {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Error building account: {:?}", e);
                continue;
            }
        };
        println!("account: {:?}", account.id().to_hex());

        let is_match = matches_pattern(&account.id().to_hex().to_string(), user_pattern);

        if is_match {
            println!("Match found");
            println!("anchor block: {:?}", anchor_block.block_num());
            println!("init_seed: {:?}", init_seed);
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // cargo test --release --bin vanity_account
    #[test]
    fn test_pattern_to_regex_matches() {
        let address = "0xabc925f3e6ef7f1000049c6efc02ef";
        let pattern = "0xabcXXXXXXXXXXXXXXXXXXXXXXXXXXX";

        let result = matches_pattern(address, pattern);

        println!("result: {:?}", result);
        assert!(result, "Expected the pattern to match, but it did not!");
    }
}
