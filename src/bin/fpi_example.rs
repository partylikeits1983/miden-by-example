use std::{fs, path::Path, sync::Arc};

use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use tokio::time::Duration;

use miden_client::{
    account::{Account, AccountCode, AccountId, AccountStorageMode, AccountType},
    asset::AssetVault,
    crypto::RpoRandomCoin,
    rpc::{
        domain::account::{AccountDetails, AccountStorageRequirements},
        Endpoint, TonicRpcClient,
    },
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{ForeignAccount, TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};

use miden_objects::{
    account::{AccountBuilder, AccountComponent, AccountStorage, AuthSecretKey, StorageSlot},
    assembly::Assembler,
    crypto::dsa::rpo_falcon512::SecretKey,
    Word,
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
    // STEP 1: Create the count reader contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating count reader contract.");

    // Load the MASM file for the counter contract
    let file_path = Path::new("./masm/accounts/count_reader.masm");
    let raw_account_code = fs::read_to_string(file_path).unwrap();

    // Define the Counter Contract account id from counter contract deploy
    let counter_contract_id = AccountId::from_hex("0x05a4e16cfd9bd2000001a4bdfe5066").unwrap();
    let get_count_root = "0x92495ca54d519eb5e4ba22350f837904d3895e48d74d8079450f19574bb84cb6";

    let account_code = raw_account_code
        .replace("{get_count_proc_root}", &get_count_root)
        .replace(
            "{account_id_prefix}",
            &counter_contract_id.prefix().to_string(),
        )
        .replace(
            "{account_id_suffix}",
            &counter_contract_id.suffix().to_string(),
        );

    // println!("Final acount code:\n{}", account_code);

    // Prepare assembler (debug mode = true)
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    // Compile the account code into `AccountComponent` with one storage slot
    let account_component = AccountComponent::compile(
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
    let (count_reader_contract, counter_seed) = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(account_component.clone())
        .build()
        .unwrap();

    println!("contract id: {:?}", count_reader_contract.id().to_hex());
    println!("account_storage: {:?}", count_reader_contract.storage());

    // Since the counter contract is public and does sign any transactions, auth_secret_key is not required.
    // However, to import to the client, we must generate a random value.
    let (_counter_pub_key, auth_secret_key) = get_new_pk_and_authenticator();

    client
        .add_account(
            &count_reader_contract.clone(),
            Some(counter_seed),
            &auth_secret_key,
            false,
        )
        .await
        .unwrap();

    // Print the procedure root hash
    let get_proc_export = account_component
        .library()
        .exports()
        .find(|export| export.name.as_str() == "copy_count")
        .unwrap();

    let get_proc_mast_id = account_component
        .library()
        .get_export_node_id(get_proc_export);

    let proc_root = account_component
        .library()
        .mast_forest()
        .get_node_by_id(get_proc_mast_id)
        .unwrap()
        .digest()
        .to_hex();

    println!("copy_count procedure root: {:?}", proc_root);

    // -------------------------------------------------------------------------
    // STEP 2: Get Counter Contract
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Building counter contract from public state");

    let account_details = client
        .test_rpc_api()
        .get_account_update(counter_contract_id)
        .await
        .unwrap();

    let AccountDetails::Public(counter_contract_details, _) = account_details else {
        panic!("counter contract must be public");
    };

    // Getting the value of the count from slot 0 and the nonce of the counter contract
    let count_value = counter_contract_details.storage().slots().get(0).unwrap();
    let counter_nonce = counter_contract_details.nonce();

    println!("count val: {:?}", count_value.value());
    println!("counter nonce: {:?}", counter_nonce);

    // Load the MASM file for the counter contract
    let file_path = Path::new("./masm/accounts/counter.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    // Prepare assembler (debug mode = true)
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    // Compile the account code into `AccountComponent` with the count value returned by the node
    let counter_component = AccountComponent::compile(
        account_code,
        assembler,
        vec![StorageSlot::Value(count_value.value())],
    )
    .unwrap()
    .with_supports_all_types();

    // Initialize the AccountStorage with the count value returned by the node
    let account_storage =
        AccountStorage::new(vec![StorageSlot::Value(count_value.value())]).unwrap();

    // Build AccountCode from components
    let account_code = AccountCode::from_components(
        &[counter_component.clone()],
        AccountType::RegularAccountImmutableCode,
    )
    .unwrap();

    // The counter contract doesn't have any assets so we pass an empty vector
    let vault = AssetVault::new(&[]).unwrap();

    // Build the counter contract from parts
    let counter_contract = Account::from_parts(
        counter_contract_id,
        vault,
        account_storage,
        account_code,
        counter_nonce,
    );

    // -------------------------------------------------------------------------
    // STEP 3: Call the Counter Contract via FPI
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Call Counter Copy Contract With Script");

    // Load the MASM script referencing the increment procedure
    let file_path = Path::new("./masm/scripts/reader_script.masm");
    let original_code = fs::read_to_string(file_path).unwrap();

    // I need to replace {get_count} and {account_id}
    let replaced_code = original_code.replace("{copy_count}", &proc_root);

    // Compile the script referencing our procedure
    let tx_script = client.compile_tx_script(vec![], &replaced_code).unwrap();

    let foreign_account =
        ForeignAccount::public(counter_contract_id, AccountStorageRequirements::default()).unwrap();
    // let foreign_account = ForeignAccount::public(counter_contract_id, AccountStorageRequirements::default()).unwrap();

    // Build a transaction request with the custom script
    let tx_request = TransactionRequestBuilder::new()
        .with_foreign_accounts([foreign_account])
        .with_custom_script(tx_script)
        .unwrap()
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(count_reader_contract.id(), tx_request)
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
