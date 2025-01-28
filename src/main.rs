use std::{fs, path::Path, sync::Arc};

use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

use miden_client::{
    account::{AccountStorageMode, AccountType},
    crypto::RpoRandomCoin,
    rpc::Endpoint,
    rpc::TonicRpcClient,
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};

use miden_objects::{
    account::{AccountBuilder, AccountComponent, AuthSecretKey, StorageSlot},
    assembly::Assembler,
    crypto::{dsa::rpo_falcon512::SecretKey, hash::rpo::RpoDigest},
    Word,
};

pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
    // Load default store and RPC config, or replace this with actual config loading
    let store_path = "store.sqlite3";

    // https://rpc.devnet.miden.io:443
    let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.devnet.miden.io".to_string(),
        Some(443),
    );
    let timeout_ms = 10_000;

    // Create the SQLite store
    let store = SqliteStore::new(store_path.into())
        .await
        .map_err(ClientError::StoreError)?;

    let arc_store = Arc::new(store);

    // Seed the RNG
    let mut seed_rng = rand::thread_rng();
    let coin_seed: [u64; 4] = seed_rng.gen();

    // Create the random coin instance
    let rng = RpoRandomCoin::new(coin_seed.map(Felt::new));

    // Create the authenticator referencing the same store and RNG
    let authenticator = StoreAuthenticator::new_with_rng(arc_store.clone(), rng.clone());

    // Build the RPC client
    let rpc_client = Box::new(TonicRpcClient::new(endpoint, timeout_ms));

    // Finally, instantiate the client. The `in_debug_mode` value can be toggled as needed.
    let client = Client::new(
        rpc_client,
        rng,
        arc_store,
        Arc::new(authenticator),
        /* in_debug_mode = */ false,
    );

    Ok(client)
}

pub fn get_new_pk_and_authenticator() -> (Word, AuthSecretKey) {
    // Create a deterministic RNG with a zeroed seed for this example.
    let seed = [0_u8; 32];
    let mut rng = ChaCha20Rng::from_seed(seed);

    // Generate a new Falcon-512 secret key.
    let sec_key = SecretKey::with_rng(&mut rng);

    // Convert the Falcon-512 public key into a `Word` (a 4xFelt representation).
    let pub_key: Word = sec_key.public_key().into();

    // Wrap the secret key in an `AuthSecretKey` for account authentication.
    let auth_secret_key = AuthSecretKey::RpoFalcon512(sec_key);

    (pub_key, auth_secret_key)
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

    // -------------------------------------------------------------------------
    // STEP 1: Create a basic counter contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating Counter Contract.");

    // 1A) Load the MASM file containing an account definition (e.g. a 'counter' contract).
    let file_path = Path::new("./masm/accounts/counter.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    // 1B) Prepare the assembler for compiling contract code (debug mode = true).
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    // 1C) Compile the account code into an `AccountComponent`
    //     and initialize it with one storage slot (for our counter).
    let account_component = AccountComponent::compile(
        account_code,
        assembler,
        vec![StorageSlot::Value(Word::default())],
    )
    .unwrap()
    .with_supports_all_types();

    // 1D) Build a new account for the counter contract, retrieve the account, seed, and secret key.
    // let (counter_contract, counter_seed, auth_secret_key) = create_new_account(account_component);

    let (_counter_pub_key, auth_secret_key) = get_new_pk_and_authenticator();
    let (_counter_pub_key_1, _auth_secret_key_1) = get_new_pk_and_authenticator();

    // let mut init_seed = [0u8; 32];
    let init_seed = ChaCha20Rng::from_entropy().gen();

    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    // Build a new `Account` using the provided component plus the Falcon-512 verifier.
    // Uses a random seed for the account’s RNG.
    let (counter_contract, counter_seed) = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode) // account type
        .storage_mode(AccountStorageMode::Public) // storage mode
        .with_component(account_component) // main contract logic
        .build()
        .unwrap();

    println!(
        "counter_contract hash: {:?}",
        counter_contract.hash().to_hex()
    );
    println!("contract id: {:?}", counter_contract.id().to_hex());

    client
        .add_account(
            &counter_contract.clone(),
            Some(counter_seed),
            &_auth_secret_key_1,
            false,
        )
        .await
        .unwrap();

    // 1F) Print out procedure root hashes for debugging/inspection.
    let procedures = counter_contract.code().procedure_roots();
    let procedures_vec: Vec<RpoDigest> = procedures.collect();
    for (index, procedure) in procedures_vec.iter().enumerate() {
        println!("Procedure {}: {:?}", index + 1, procedure.to_hex());
    }
    println!("number of procedures: {}", procedures_vec.len());

    // -------------------------------------------------------------------------
    // STEP 2: Call the Counter Contract with a script
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Call Counter Contract With Script");

    for _i in 0..1000 {
        client.sync_state().await.unwrap();

        // 2A) Grab the compiled procedure hash (in this case, the first procedure).
        let procedure_2_hash = procedures_vec[0].to_hex();
        let procedure_call = format!("{}", procedure_2_hash);

        // 2B) Load a MASM script that will reference our increment procedure.
        let file_path = Path::new("./masm/scripts/counter_script.masm");
        let original_code = fs::read_to_string(file_path).unwrap();

        // 2C) Replace the placeholder `{increment_count}` in the script with the actual procedure call.
        let replaced_code = original_code.replace("{increment_count}", &procedure_call);
        // println!("Final script:\n{}", replaced_code);

        // 2D) Compile the script (which now references our procedure).
        let tx_script = client.compile_tx_script(vec![], &replaced_code).unwrap();

        // 2E) Build a transaction request using the custom script.
        let tx_increment_request = TransactionRequestBuilder::new()
            .with_custom_script(tx_script)
            .unwrap()
            .build();

        // 2F) Execute the transaction locally (producing a result).
        let tx_result = client
            .new_transaction(counter_contract.id(), tx_increment_request)
            .await
            .unwrap();

        let tx_id = tx_result.executed_transaction().id();
        /*         println!(
            "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
            tx_id
        ); */

        // 2G) Submit the transaction to the network.
        let _ = client.submit_transaction(tx_result).await;

        // Wait a bit for the network to process the transaction, then re-sync.
        client.sync_state().await.unwrap();

        // 2H) Retrieve the updated contract data and observe the incremented counter.
        let account = client.get_account(counter_contract.id()).await.unwrap();
        println!(
            "storage item 0: {:?}",
            account.unwrap().account().storage().get_item(0)
        );
    }
    Ok(())
}
