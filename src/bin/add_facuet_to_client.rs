use std::{fs, path::Path, sync::Arc};

use rand::Rng;
use rand_chacha::{rand_core::SeedableRng, ChaCha20Rng};

use miden_client::{
    account::{
        component::{BasicWallet, RpoFalcon512},
        AccountBuilder, AccountId, AccountStorageMode, AccountType,
    },
    asset::FungibleAsset,
    crypto::RpoRandomCoin,
    note::{Note, NoteType},
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt,
};

use miden_objects::{
    account::{AccountComponent, AccountStorage, AuthSecretKey, StorageSlot},
    assembly::Assembler,
    crypto::{dsa::rpo_falcon512::SecretKey, hash::rpo::RpoDigest},
    Word,
};

use miden_lib::account::faucets::create_basic_fungible_faucet;

// Initialize client helper
pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
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

// Helper to create keys & authenticator
pub fn get_new_pk_and_authenticator() -> (Word, AuthSecretKey) {
    let seed = [0_u8; 32];
    let mut rng = ChaCha20Rng::from_seed(seed);

    let sec_key = SecretKey::with_rng(&mut rng);
    let pub_key: Word = sec_key.public_key().into();
    let auth_secret_key = AuthSecretKey::RpoFalcon512(sec_key);

    (pub_key, auth_secret_key)
}

// Helper to create a basic account (for Alice and Bob)
async fn create_basic_account(
    client: &mut Client<RpoRandomCoin>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = SecretKey::with_rng(client.rng());
    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key()))
        .with_component(BasicWallet);
    let (account, seed) = builder.build().unwrap();
    client
        .add_account(
            &account,
            Some(seed),
            &AuthSecretKey::RpoFalcon512(key_pair),
            false,
        )
        .await?;
    Ok(account)
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
    // STEP 1: Create two basic accounts (Alice and Bob)
    //------------------------------------------------------------
    println!("\n[STEP 1] Creating new accounts");
    let alice_account = create_basic_account(&mut client).await?;
    println!("Alice's account ID: {:?}", alice_account.id().to_hex());

    //------------------------------------------------------------
    // STEP 2: Mint and consume tokens for Alice with Ephemeral P2ID
    //------------------------------------------------------------
    println!("\n[STEP 2] Mint tokens with Ephemeral P2ID");

    let faucet_id = AccountId::from_hex("0xf42ab06ffd227c2000005b2767bc5e").unwrap();

    let account_details = client
        .test_rpc_api()
        .get_account_update(faucet_id)
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

    /*     // Prepare assembler (debug mode = true)
       let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

       // Compile the account code into `AccountComponent` with the count value returned by the node
       let counter_component = AccountComponent::compile(
         // create_basic_fungible_faucet,
           assembler,
           vec![StorageSlot::Value(count_value.value())],
       )
       .unwrap()
       .with_supports_all_types();
    */
    let amount: u64 = 100;
    let fungible_asset_mint_amount = FungibleAsset::new(faucet_id, amount).unwrap();

    let transaction_request = TransactionRequestBuilder::mint_fungible_asset(
        fungible_asset_mint_amount.clone(),
        alice_account.id(),
        NoteType::Public,
        client.rng(),
    )
    .unwrap()
    .build();

    let tx_execution_result = client
        .new_transaction(faucet_id, transaction_request)
        .await?;
    client
        .submit_transaction(tx_execution_result.clone())
        .await?;

    Ok(())
}
