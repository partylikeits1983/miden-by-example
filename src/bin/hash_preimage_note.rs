use std::{fs, path::Path, sync::Arc};

use miden_crypto::rand::FeltRng;
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use tokio::time::Duration;

use miden_client::{
    account::{
        component::{BasicFungibleFaucet, BasicWallet, RpoFalcon512},
        AccountBuilder, AccountStorageMode, AccountType,
    },
    asset::{FungibleAsset, TokenSymbol},
    crypto::RpoRandomCoin,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};
use miden_crypto::hash::rpo::Rpo256 as Hasher;

use miden_objects::{
    account::AuthSecretKey, assembly::Assembler, crypto::dsa::rpo_falcon512::SecretKey, Word,
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
    // STEP 1: Create a basic account for Alice
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

    let (bob_account, seed) = builder.build().unwrap();

    // Add the account to the client
    client
        .add_account(
            &bob_account,
            Some(seed),
            &AuthSecretKey::RpoFalcon512(key_pair),
            false,
        )
        .await?;

    println!("Bob's account ID: {:?}", bob_account.id().to_hex());

    //------------------------------------------------------------
    // STEP 2: Deploy a fungible faucet
    //------------------------------------------------------------
    println!("\n[STEP 2] Deploying a new fungible faucet.");

    // Faucet seed
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    // Anchor block
    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    // Faucet parameters
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000);

    // Generate key pair
    let key_pair = SecretKey::with_rng(client.rng());

    // Build the account
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key()))
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap());

    let (faucet_account, seed) = builder.build().unwrap();

    // Add the faucet to the client
    client
        .add_account(
            &faucet_account,
            Some(seed),
            &AuthSecretKey::RpoFalcon512(key_pair),
            false,
        )
        .await?;

    println!("Faucet account ID: {:?}", faucet_account.id().to_hex());

    // Resync to show newly deployed faucet
    tokio::time::sleep(Duration::from_secs(2)).await;
    client.sync_state().await?;

    //------------------------------------------------------------
    // STEP 3: Mint and consume tokens for Alice
    //------------------------------------------------------------
    println!("\n[STEP 3] Mint tokens");

    let amount: u64 = 100;
    let fungible_asset_mint_amount = FungibleAsset::new(faucet_account.id(), amount).unwrap();

    let transaction_request = TransactionRequestBuilder::mint_fungible_asset(
        fungible_asset_mint_amount.clone(),
        alice_account.id(),
        NoteType::Public,
        client.rng(),
    )
    .unwrap()
    .build();
    let tx_execution_result = client
        .new_transaction(faucet_account.id(), transaction_request)
        .await?;

    client
        .submit_transaction(tx_execution_result.clone())
        .await?;

    let note_id_mint = tx_execution_result.created_notes().get_note(0).id();

    // consuming
    loop {
        // Resync to get the latest data
        client.sync_state().await?;

        let consumable_notes = client
            .get_consumable_notes(Some(alice_account.id()))
            .await?;
        let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();
        println!("number of notes: {:?}", list_of_note_ids.len());

        // Check if note_id_mint exists in the list_of_note_ids vector
        if list_of_note_ids.contains(&note_id_mint) {
            let transaction_request =
                TransactionRequestBuilder::consume_notes(vec![note_id_mint]).build();
            let tx_execution_result = client
                .new_transaction(alice_account.id(), transaction_request)
                .await?;

            let delta = tx_execution_result.account_delta();
            println!("delta: {:?}", delta);

            client.submit_transaction(tx_execution_result).await?;
            break;
        } else {
            println!("waiting");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 4: Create counter contract increment NOTE using voter account
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Create note");

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

    // Load the MASM script referencing the increment procedure
    let file_path = Path::new("./masm/notes/hash_preimage_note.masm");
    let original_code = fs::read_to_string(file_path).unwrap();

    let hash_str: Vec<Felt> = digest.to_vec(); // Example vector, replace with your actual data
    let formatted_str: String = hash_str
        .iter()
        .map(|felt| felt.to_string())
        .collect::<Vec<String>>()
        .join(".");

    println!("{}", formatted_str);

    // Replace the placeholder with the actual procedure call
    let replaced_code = original_code.replace("{digest}", &formatted_str);
    println!("Final script:\n{}", replaced_code);

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

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
    let vault = NoteAssets::new(vec![fungible_asset_mint_amount.clone().into()])?;

    // Building note
    let increment_note = Note::new(vault, metadata, recipient);
    println!("note hash: {:?}", increment_note.hash());

    let output_note = OutputNote::Full(increment_note.clone());

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
    // STEP 5: Consume Note
    // -------------------------------------------------------------------------

    let secret = [Felt::new(0), Felt::new(3), Felt::new(0), Felt::new(3)];
    // let note_args = secret;

    let tx_note_consume_request = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(increment_note.id(), Some(secret))])
        .build();

    // Execute the transaction locally
    let tx_result = client
        .new_transaction(bob_account.id(), tx_note_consume_request)
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

    Ok(())
}
