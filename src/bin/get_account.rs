use std::sync::Arc;

use rand::Rng;

use miden_client::{
    account::AccountId,
    crypto::RpoRandomCoin,
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    Client, ClientError, Felt,
};

pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
    // RPC endpoint and timeout
    let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let timeout_ms = 10_000;

    // Build RPC client
    let rpc_api = Box::new(TonicRpcClient::new(endpoint, timeout_ms));

    // Seed RNG
    let mut seed_rng = rand::thread_rng();
    let coin_seed: [u64; 4] = seed_rng.gen();

    // Create random coin instance
    let rng = RpoRandomCoin::new(coin_seed.map(Felt::new));

    // SQLite path
    let store_path = "store.sqlite3";

    // Initialize SQLite store
    let store = SqliteStore::new(store_path.into())
        .await
        .map_err(ClientError::StoreError)?;
    let arc_store = Arc::new(store);

    // Create authenticator referencing the store and RNG
    let authenticator = StoreAuthenticator::new_with_rng(arc_store.clone(), rng.clone());

    // Instantiate client (toggle debug mode as needed)
    let client = Client::new(rpc_api, rng, arc_store, Arc::new(authenticator), true);

    Ok(client)
}

#[tokio::main]
async fn main() -> Result<(), ClientError> {
    // -------------------------------------------------------------------------
    // Initialize the Miden client
    // -------------------------------------------------------------------------
    let mut client = initialize_client().await?;
    println!("Client initialized successfully.");

    // Fetch and display the latest synchronized block number from the node.
    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let account_id = AccountId::from_hex("0xeefc88a49371b60000012e5ae8ea9b").unwrap();

    let account_details = client
        .test_rpc_api()
        .get_account_update(account_id)
        .await
        .unwrap();

    match account_details {
        miden_client::rpc::domain::account::AccountDetails::Public(account, _) => {
            println!("Account Storage: {:?}", account.storage().slots());
        }
        miden_client::rpc::domain::account::AccountDetails::Private(_, _) => {
            println!("This is a private account. Storage is not available.");
        }
    }

    Ok(())
}
