use std::{fs, path::Path, sync::Arc};

use miden_crypto::rand::FeltRng;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use tokio::time::Duration;

use miden_client::{
    account::{
        component::{BasicWallet, RpoFalcon512},
        Account, AccountBuilder, AccountCode, AccountId, AccountStorageMode, AccountType,
    },
    asset::AssetVault,
    crypto::RpoRandomCoin,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    rpc::{domain::account::AccountDetails, Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};

use miden_objects::{
    account::{AccountComponent, AccountStorage, AuthSecretKey, StorageSlot},
    assembly::Assembler,
    crypto::{dsa::rpo_falcon512::SecretKey, hash::rpo::RpoDigest},
    Word,
};

pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
    // RPC endpoint and timeout

    let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    // let endpoint = Endpoint::new("http".to_string(), "localhost".to_string(), Some(57291));
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

    //------------------------------------------------------------
    // STEP 1: Create a basic account for user
    //------------------------------------------------------------
    println!("\n[STEP 1] Creating a new account for Alice");

    // Account seed
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    // Generate key pair
    let key_pair = SecretKey::with_rng(client.rng());

    // Anchor block
    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    // Build the account
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key()))
        .with_component(BasicWallet);

    let (alice_account, seed) = builder.build().unwrap();

    // Add the account to the client
    client
        .add_account(
            &alice_account,
            Some(seed),
            &AuthSecretKey::RpoFalcon512(key_pair),
            false,
        )
        .await?;

    println!("Alice's account ID: {:?}", alice_account.id().to_hex());

    // -------------------------------------------------------------------------
    // STEP 2: Build Counter Contract From Public State
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Building counter contract from public state");

    // Define the Counter Contract account id from counter contract deploy
    let counter_contract_id = AccountId::from_hex("0x95cef1051e74fb000005b23f08c5eb").unwrap();

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
        assembler.clone(),
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

    // Since the counter contract is public and does sign any transactions, auth_secret_key is not required.
    // However, to import to the client, we must generate a random value.
    let (_, _auth_secret_key) = get_new_pk_and_authenticator();

    client
        .add_account(&counter_contract.clone(), None, &_auth_secret_key, true)
        .await
        .unwrap();

    // -------------------------------------------------------------------------
    // STEP 3: Create counter contract increment NOTE with user account
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Call the increment_count procedure in the counter contract");

    // Print procedure root hashes
    let procedures = counter_contract.code().procedure_roots();
    let procedures_vec: Vec<RpoDigest> = procedures.collect();
    for (index, procedure) in procedures_vec.iter().enumerate() {
        println!("Procedure {}: {:?}", index + 1, procedure);
    }
    println!("number of procedures: {}", procedures_vec.len());

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

    let increment_count_root_1 = counter_component
        .library()
        .mast_forest()
        .get_node_by_id(get_increment_count_mast_id)
        .unwrap()
        .digest();

    println!("proc root: {:?}", increment_count_root_1);

    // Load the MASM script referencing the increment procedure
    let file_path = Path::new("./masm/notes/increment_note.masm");
    let original_code = fs::read_to_string(file_path).unwrap();

    // Replace the placeholder with the actual procedure call
    let replaced_code = original_code.replace("{increment_count}", &increment_count_root);
    println!("Final script:\n{}", replaced_code);

    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(replaced_code, assembler).unwrap();
    let note_inputs = NoteInputs::new(vec![]).unwrap();

    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs);
    let tag = NoteTag::for_public_use_case(0, 0, NoteExecutionMode::Local).unwrap();

    let aux = Felt::new(0);
    let metadata = NoteMetadata::new(
        alice_account.id(),
        NoteType::Public,
        tag,
        NoteExecutionHint::always(),
        aux,
    )?;
    let vault = NoteAssets::new(vec![])?;

    // Building note
    let increment_note = Note::new(vault, metadata, recipient);
    println!("note hash: {:?}", increment_note.hash());

    let output_note = OutputNote::Full(increment_note.clone());

    // Build a transaction request with the custom script

    let incr_note_create_request = TransactionRequestBuilder::new()
        .with_own_output_notes([output_note].to_vec())
        .unwrap()
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(alice_account.id(), incr_note_create_request)
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
    println!("waiting 10 seconds");
    tokio::time::sleep(Duration::from_secs(10)).await;
    client.sync_state().await.unwrap();

    // -------------------------------------------------------------------------
    // STEP 4: Consume Increment Note with Counter Contract
    // -------------------------------------------------------------------------

    let tx_note_consume_request = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(increment_note.id(), Some(Word::default()))])
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(counter_contract_details.id(), tx_note_consume_request)
        .await
        .unwrap();

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;
    println!("waiting 10 seconds");
    tokio::time::sleep(Duration::from_secs(10)).await;
    client.sync_state().await.unwrap();

    // Wait, then re-sync
    /*     // Retrieve updated contract data to see the incremented counter
    let account = client.get_account(counter_contract.id()).await.unwrap();
    println!(
        "counter contract storage: {:?}",
        account.unwrap().account().storage().get_item(0)
    ); */

    Ok(())
}
