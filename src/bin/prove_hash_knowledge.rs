use std::{fs, path::Path, sync::Arc};

use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

use miden_client::{
    account::{AccountStorageMode, AccountType},
    crypto::RpoRandomCoin,
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};

use miden_crypto::hash::rpo::Rpo256 as Hasher;
use miden_objects::{
    account::{AccountBuilder, AccountComponent, AuthSecretKey, StorageSlot},
    assembly::Assembler,
    crypto::dsa::rpo_falcon512::SecretKey,
    Word,
};

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
    let mut client = initialize_client().await?;

    // -------------------------------------------------------------------------
    // STEP 1: Create a basic contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating contract.");

    // Load the MASM file for the contract
    let file_path = Path::new("./masm/accounts/hash_preimage_knowledge.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    // Prepare assembler (debug mode = true)
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    // Hashing Secret number combination
    let elements = [
        Felt::new(1),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(1),
    ];
    let digest = Hasher::hash_elements(&elements);
    println!("digest: {:?}", digest);

    // Compile the account code into `AccountComponent` with one storage slot
    let contract_component =
        AccountComponent::compile(account_code, assembler, vec![StorageSlot::Value(*digest)])
            .unwrap()
            .with_supports_all_types();

    // Init seed for the contract
    let init_seed = ChaCha20Rng::from_entropy().gen();

    // Anchor block of the account
    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    // Build the new `Account` with the component
    let (contract, seed) = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(contract_component.clone())
        .build()
        .unwrap();

    println!("contract id: {:?}", contract.id().to_hex());
    println!("account_storage: {:?}", contract.storage());

    // Since the contract is public and does sign any transactions, auth_secret_key is not required.
    // However, to import to the client, we must generate a random value.
    let (_pub_key, auth_secret_key) = get_new_pk_and_authenticator();

    client
        .add_account(&contract.clone(), Some(seed), &auth_secret_key, false)
        .await
        .unwrap();

    // Print the procedure root hash
    let get_proc_export = contract_component
        .library()
        .exports()
        .find(|export| export.name.as_str() == "prove_hash")
        .unwrap();

    let get_proc_mast_id = contract_component
        .library()
        .get_export_node_id(get_proc_export);

    let proc_hash = contract_component
        .library()
        .mast_forest()
        .get_node_by_id(get_proc_mast_id)
        .unwrap()
        .digest()
        .to_hex();

    println!("prove_hash procedure hash: {:?}", proc_hash);

    // -------------------------------------------------------------------------
    // STEP 2: Call the Contract with a script
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Call Contract With Script");

    // Load the MASM script referencing the increment procedure
    let file_path = Path::new("./masm/scripts/hash_knowledge_script.masm");
    let original_code = fs::read_to_string(file_path).unwrap();

    // Replace the placeholder with the actual procedure call
    let replaced_code = original_code.replace("{prove_hash}", &proc_hash);
    println!("Final script:\n{}", replaced_code);

    // Compile the script referencing our procedure
    let tx_script = client.compile_tx_script(vec![], &replaced_code).unwrap();

    // Build a transaction request with the custom script
    let tx_increment_request = TransactionRequestBuilder::new()
        .with_custom_script(tx_script)
        .unwrap()
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(contract.id(), tx_increment_request)
        .await
        .unwrap();

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;

    client.sync_state().await.unwrap();

    Ok(())
}
