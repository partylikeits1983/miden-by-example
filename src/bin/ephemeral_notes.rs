use std::sync::Arc;

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
    note::{create_p2id_note, Note, NoteType},
    rpc::{Endpoint, TonicRpcClient},
    store::{sqlite_store::SqliteStore, StoreAuthenticator},
    transaction::{OutputNote, TransactionRequestBuilder},
    utils::{Deserializable, Serializable},
    Client, ClientError, Felt,
};

use miden_objects::{account::AuthSecretKey, crypto::dsa::rpo_falcon512::SecretKey, Word};

pub async fn initialize_client() -> Result<Client<RpoRandomCoin>, ClientError> {
    // RPC endpoint and timeout
    // let endpoint = Endpoint::new("http".to_string(), "localhost".to_string(), Some(57291));
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
    let mut seed_rng = rand::thread_rng();
    let seed: [u8; 32] = seed_rng.gen();
    let mut rng = ChaCha20Rng::from_seed(seed);

    let sec_key = SecretKey::with_rng(&mut rng);
    let pub_key: Word = sec_key.public_key().into();

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
    // STEP 1: Deploy a fungible faucet
    //------------------------------------------------------------
    println!("\n[STEP 1] Deploying a new fungible faucet.");

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
    // STEP 2: Create basic wallet accounts
    //------------------------------------------------------------
    println!("\n[STEP 2] Creating new accounts ");

    let mut accounts = vec![];
    let mut seeds = vec![];
    let mut key_pairs = vec![];

    for _ in 0..5 {
        let init_seed = ChaCha20Rng::from_entropy().gen();

        let key_pair = SecretKey::with_rng(client.rng());

        let builder = AccountBuilder::new(init_seed)
            .anchor((&anchor_block).try_into().unwrap())
            .account_type(AccountType::RegularAccountUpdatableCode)
            .storage_mode(AccountStorageMode::Public)
            .with_component(RpoFalcon512::new(key_pair.public_key()))
            .with_component(BasicWallet);

        let (account, seed) = builder.build().unwrap();

        accounts.push(account.clone());
        key_pairs.push(key_pair.clone());
        seeds.push(seed.clone());

        println!("account id: {:?}", account.id().to_hex());

        client
            .add_account(
                &account,
                Some(seed),
                &AuthSecretKey::RpoFalcon512(key_pair.clone()),
                true,
            )
            .await?;
    }

    // Accounts (for demo purposes)
    let alice = &accounts[0];
    let bob = &accounts[1];
    let charlie = &accounts[2];
    let dave = &accounts[3];
    let sybil = &accounts[4];

    //------------------------------------------------------------
    // STEP 3: Mint and consume tokens for Alice
    //------------------------------------------------------------
    println!("\n[STEP 3] Mint tokens");

    let amount: u64 = 100;
    let fungible_asset_mint_amount = FungibleAsset::new(faucet_account.id(), amount).unwrap();

    let transaction_request = TransactionRequestBuilder::mint_fungible_asset(
        fungible_asset_mint_amount.clone(),
        alice.id(),
        NoteType::Public,
        client.rng(),
    )
    .unwrap()
    .build();
    let tx_execution_result = client
        .new_transaction(faucet_account.id(), transaction_request)
        .await?;

    client.submit_transaction(tx_execution_result).await?;

    // consuming
    loop {
        // Resync to get the latest data
        client.sync_state().await?;

        let consumable_notes = client.get_consumable_notes(Some(alice.id())).await?;
        let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();

        if list_of_note_ids.len() == 1 {
            let transaction_request =
                TransactionRequestBuilder::consume_notes(list_of_note_ids).build();
            let tx_execution_result = client
                .new_transaction(alice.id(), transaction_request)
                .await?;

            let delta = tx_execution_result.account_delta();
            println!("delta: {:?}", delta);

            client.submit_transaction(tx_execution_result).await?;
            break;
        } else {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_state().await?;

    // sanity check
    loop {
        client.sync_state().await?;
        let alice = client.get_account(alice.id()).await.unwrap();

        let balance = alice
            .unwrap()
            .account()
            .vault()
            .get_balance(faucet_account.id())
            .unwrap();

        if balance != 0 {
            println!("balance: {:?}", balance);
            break;
        }
        println!("waiting");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    //------------------------------------------------------------
    // STEP 4: Creating ephemeral P2ID note
    //------------------------------------------------------------
    println!("\n[STEP 4] Creating ephemeral note ");

    let mut ephemeral_p2id_notes = vec![];

    for i in 0..4 {
        println!("sender: {:?}", accounts[i].id().to_hex());
        println!("target: {:?}", accounts[i + 1].id().to_hex());

        // 1 create p2id note
        let send_amount = 20;
        let fungible_asset_send_amount =
            FungibleAsset::new(faucet_account.id(), send_amount).unwrap();

        let p2id_note = create_p2id_note(
            accounts[i].id(),
            accounts[i + 1].id(),
            vec![fungible_asset_send_amount.into()],
            NoteType::Public,
            Felt::new(0),
            client.rng(),
        )
        .unwrap();

        ephemeral_p2id_notes.push(p2id_note.clone());

        let output_note = OutputNote::Full(p2id_note.clone());

        let transaction_request = TransactionRequestBuilder::new()
            .with_own_output_notes(vec![output_note])
            .unwrap()
            .build();

        let tx_execution_result = client
            .new_transaction(accounts[i].id(), transaction_request)
            .await?;

        // The P2ID note has to have been submitted from Alice to Bob
        client.submit_transaction(tx_execution_result).await?;

        // send the p2id note "over the wire"
        let serialized = p2id_note.to_bytes();
        let deserialized_p2id_note = Note::read_from_bytes(&serialized).unwrap();
        println!("original: {:?}", p2id_note.hash());
        println!("deserialized: {:?}", deserialized_p2id_note.hash());

        // Then here, Bob can already start to consume it before it even lands in the block
        let consume_note_request =
            TransactionRequestBuilder::consume_notes(vec![deserialized_p2id_note.id()])
                .with_unauthenticated_input_notes([(deserialized_p2id_note, None)])
                .build();

        let tx_execution_result = client
            .new_transaction(accounts[i + 1].id(), consume_note_request)
            .await?;

        client
            .submit_transaction(tx_execution_result.clone())
            .await?;

        // end of tx chain
    }

    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_state().await?;

    for account in accounts.clone() {
        let new_account = client.get_account(account.id()).await.unwrap();
        let balance = new_account
            .unwrap()
            .account()
            .vault()
            .get_balance(faucet_account.id())
            .unwrap();

        println!("account balance balance: {:?}", balance);
    }

    Ok(())
}
