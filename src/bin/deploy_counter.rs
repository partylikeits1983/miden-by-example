use std::{fs, path::Path, sync::Arc};

use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use tokio::time::Duration;

use miden_client::{
    account::{AccountStorageMode, AccountType},
    crypto::RpoRandomCoin,
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};

use miden_objects::{
    account::{AccountBuilder, AccountComponent, AuthSecretKey, StorageSlot},
    assembly::Assembler,
    crypto::dsa::rpo_falcon512::SecretKey,
    Word,
};

pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
    // RPC endpoint and timeout
    /*     let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    ); */
    let endpoint = Endpoint::new("http".to_string(), "localhost".to_string(), Some(57291));
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

pub fn get_new_pk_and_authenticator() -> (Word, AuthSecretKey) {
    // Create a deterministic RNG with zeroed seed
    let seed = [0_u8; 32];
    let mut rng = ChaCha20Rng::from_seed(seed);

    // Generate Falcon-512 secret key
    let sec_key = SecretKey::with_rng(&mut rng);

    // Convert public key to `Word` (4xFelt)
    let pub_key: Word = sec_key.public_key().into();

    // Wrap secret key in `AuthSecretKey`
    let auth_secret_key = AuthSecretKey::RpoFalcon512(sec_key);

    (pub_key, auth_secret_key)
}

#[tokio::main]
async fn main() -> Result<(), ClientError> {
    // Initialize client
    let mut client = initialize_client().await?;
    println!("Client initialized successfully.");

    // Fetch latest block from node
    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    // -------------------------------------------------------------------------
    // STEP 1: Create a basic counter contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating counter contract.");

    // Load the MASM file for the counter contract
    let file_path = Path::new("./masm/accounts/counter.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    // Prepare assembler (debug mode = true)
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    // Compile the account code into `AccountComponent` with one storage slot
    let counter_component = AccountComponent::compile(
        account_code,
        assembler,
        vec![StorageSlot::Value([
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
        ])],
    )
    .unwrap()
    .with_supports_all_types();

    // Init seed for the counter contract
    let init_seed = ChaCha20Rng::from_entropy().gen();

    // Anchor block of the account
    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    // Build the new `Account` with the component
    let (counter_contract, counter_seed) = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(counter_component.clone())
        .build()
        .unwrap();

    println!("contract id: {:?}", counter_contract.id().to_hex());
    println!("account_storage: {:?}", counter_contract.storage());

    // Since the counter contract is public and does sign any transactions, auth_secret_key is not required.
    // However, to import to the client, we must generate a random value.
    let (_counter_pub_key, auth_secret_key) = get_new_pk_and_authenticator();

    client
        .add_account(
            &counter_contract.clone(),
            Some(counter_seed),
            &auth_secret_key,
            false,
        )
        .await
        .unwrap();

    // Print the procedure root hash
    let get_increment_export = counter_component
        .library()
        .exports()
        .find(|export| export.name.as_str() == "increment_count")
        .unwrap();

    let get_increment_count_mast_id = counter_component
        .library()
        .get_export_node_id(get_increment_export);

    let increment_count_root = counter_component
        .library()
        .mast_forest()
        .get_node_by_id(get_increment_count_mast_id)
        .unwrap()
        .digest()
        .to_hex();

    println!("increment_count procedure root: {:?}", increment_count_root);

    // Print the procedure root hash
    let get_count_export = counter_component
        .library()
        .exports()
        .find(|export| export.name.as_str() == "get_count")
        .unwrap();

    let get_count_mast_id = counter_component
        .library()
        .get_export_node_id(get_count_export);

    let get_count_root = counter_component
        .library()
        .mast_forest()
        .get_node_by_id(get_count_mast_id)
        .unwrap()
        .digest()
        .to_hex();
    println!("get_count procedure root: {:?}", get_count_root);

    // -------------------------------------------------------------------------
    // STEP 2: Call the Counter Contract with a script
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Call Counter Contract With Script");

    // Load the MASM script referencing the increment procedure
    let file_path = Path::new("./masm/scripts/counter_script.masm");
    let original_code = fs::read_to_string(file_path).unwrap();

    // Replace the placeholder with the actual procedure call
    let replaced_code = original_code.replace("{increment_count}", &increment_count_root);
    println!("Final script:\n{}", replaced_code);

    // Compile the script referencing our procedure
    let tx_script = client.compile_tx_script(vec![], &replaced_code).unwrap();
    /*
    let tx_script = client
        .compile_tx_script(
            vec![(
                [Felt::new(1), Felt::new(1), Felt::new(0), Felt::new(0)],
                vec![Felt::new(1), Felt::new(1), Felt::new(0), Felt::new(0)],
            )],
            &replaced_code,
        )
        .unwrap();
    */

    // Build a transaction request with the custom script
    let tx_increment_request = TransactionRequestBuilder::new()
        .with_custom_script(tx_script)
        .unwrap()
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(counter_contract.id(), tx_increment_request)
        .await
        .unwrap();

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;

    // Wait, then re-sync
    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_state().await.unwrap();

    // Retrieve updated contract data to see the incremented counter
    let account = client.get_account(counter_contract.id()).await.unwrap();
    println!(
        "counter contract storage: {:?}",
        account.unwrap().account().storage().get_item(0)
    );

    Ok(())
}
