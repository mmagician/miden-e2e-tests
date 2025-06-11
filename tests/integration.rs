use std::sync::Arc;

use rand::random;

use miden_client::{
    ClientError, Felt, Word,
    account::{Account, AccountBuilder, AccountStorageMode, AccountType},
    asset::{Asset, FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    keystore::FilesystemKeyStore,
    note::{NoteFile, NoteType},
    transaction::{OutputNote, SwapTransactionData, TransactionRequestBuilder},
};
use miden_lib::{
    AuthScheme,
    account::{
        auth::RpoFalcon512,
        faucets::create_basic_fungible_faucet,
        wallets::{BasicWallet, create_basic_wallet},
    },
};
use miden_objects::{AccountError, account::AccountIdAnchor, crypto::dsa::rpo_falcon512};
mod util;

// use super::util::{reset_store, setup_client};
use crate::util::{InFlightSwap, reset_store, setup_client};

/// Create a new account for the matcher.
fn create_matcher_wallet(
    init_seed: [u8; 32],
    id_anchor: AccountIdAnchor,
    auth_scheme: AuthScheme,
    account_storage_mode: AccountStorageMode,
) -> Result<(Account, Word), AccountError> {
    let auth_component: RpoFalcon512 = match auth_scheme {
        AuthScheme::RpoFalcon512 { pub_key } => RpoFalcon512::new(pub_key),
    };

    let (account, account_seed) = AccountBuilder::new(init_seed)
        .anchor(id_anchor)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(account_storage_mode)
        .with_component(auth_component)
        .with_component(BasicWallet)
        // The `InFlightSwapWallet` component is not found in a `BasicWallet`
        // .with_component(AuxWallet)
        .build()?;

    Ok((account, account_seed))
}

#[tokio::test]
async fn test_matcher_swap() {
    // clean the DB for the test
    reset_store().await;

    // --------------------------------------------------------------------------------
    // Setup keys for the accounts
    // --------------------------------------------------------------------------------
    // Faucet A
    let secret_key_faucet_a = rpo_falcon512::SecretKey::new();
    let pub_key_faucet_a = secret_key_faucet_a.public_key();
    let auth_scheme_a: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_faucet_a,
    };

    // Faucet B
    let secret_key_faucet_b = rpo_falcon512::SecretKey::new();
    let pub_key_faucet_b = secret_key_faucet_b.public_key();
    let auth_scheme_b: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_faucet_b,
    };

    // Alice
    let secret_key_alice = rpo_falcon512::SecretKey::new();
    let pub_key_alice = secret_key_alice.public_key();
    let auth_scheme_alice: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_alice,
    };

    // Bob
    let secret_key_bob = rpo_falcon512::SecretKey::new();
    let pub_key_bob = secret_key_bob.public_key();
    let auth_scheme_bob: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_bob,
    };

    // Matcher
    let secret_key_matcher = rpo_falcon512::SecretKey::new();
    let pub_key_matcher = secret_key_matcher.public_key();
    let auth_scheme_matcher: AuthScheme = AuthScheme::RpoFalcon512 {
        pub_key: pub_key_matcher,
    };
    println!("Keys for accounts generated");

    // --------------------------------------------------------------------------------
    // Setup authenticator / keystore
    //
    // This needs to happen before the client is created, since we need to init the client with this authenticator.
    // --------------------------------------------------------------------------------
    // Faucet authenticator (shared for both faucets)
    let faucet_authenticator = FilesystemKeyStore::new("keystore/faucets".into()).unwrap();
    faucet_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_faucet_a))
        .unwrap();
    faucet_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_faucet_b))
        .unwrap();

    // Alice authenticator
    let alice_authenticator = FilesystemKeyStore::new("keystore/alice".into()).unwrap();
    alice_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_alice))
        .unwrap();

    // Bob authenticator
    let bob_authenticator = FilesystemKeyStore::new("keystore/bob".into()).unwrap();
    bob_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_bob))
        .unwrap();

    // Matcher authenticator
    let matcher_authenticator = FilesystemKeyStore::new("keystore/matcher".into()).unwrap();
    matcher_authenticator
        .add_key(&AuthSecretKey::RpoFalcon512(secret_key_matcher))
        .unwrap();

    // --------------------------------------------------------------------------------
    // Create client instances
    // --------------------------------------------------------------------------------
    let mut faucet_client = setup_client(Arc::new(faucet_authenticator)).await.unwrap();
    let mut alice_client = setup_client(Arc::new(alice_authenticator)).await.unwrap();
    let mut bob_client = setup_client(Arc::new(bob_authenticator)).await.unwrap();
    let mut matcher_client = setup_client(Arc::new(matcher_authenticator)).await.unwrap();

    let latest_epoch_block = faucet_client.get_latest_epoch_block().await.unwrap();
    println!("Got latest epoch block");

    // For now let's use the same max supply for both tokens
    let max_supply = Felt::new(1_000);

    let token_a_symbol = "NP";
    let token_symbol_a = TokenSymbol::try_from(token_a_symbol).unwrap();
    let token_b_symbol = "MID";
    let token_symbol_b = TokenSymbol::try_from(token_b_symbol).unwrap();

    let decimals = 2u8;

    // --------------------------------------------------------------------------------
    // Create faucet accounts
    // --------------------------------------------------------------------------------
    let (faucet_account_a, faucet_a_seed) = create_basic_fungible_faucet(
        random(),
        (&latest_epoch_block).try_into().unwrap(),
        token_symbol_a,
        decimals,
        max_supply,
        AccountStorageMode::Public,
        auth_scheme_a,
    )
    .unwrap();

    let (faucet_account_b, faucet_b_seed) = create_basic_fungible_faucet(
        random(),
        (&latest_epoch_block).try_into().unwrap(),
        token_symbol_b,
        decimals,
        max_supply,
        AccountStorageMode::Public,
        auth_scheme_b,
    )
    .unwrap();

    // --------------------------------------------------------------------------------
    // Create user/wallet accounts
    // --------------------------------------------------------------------------------
    let (alice, alice_seed) = create_basic_wallet(
        random(),
        (&latest_epoch_block).try_into().unwrap(),
        auth_scheme_alice,
        AccountType::RegularAccountImmutableCode,
        AccountStorageMode::Private,
    )
    .unwrap();

    let (bob, bob_seed) = create_basic_wallet(
        random(),
        (&latest_epoch_block).try_into().unwrap(),
        auth_scheme_bob,
        AccountType::RegularAccountImmutableCode,
        AccountStorageMode::Private,
    )
    .unwrap();

    let (matcher, matcher_seed) = create_matcher_wallet(
        random(),
        (&latest_epoch_block).try_into().unwrap(),
        auth_scheme_matcher,
        AccountStorageMode::Private,
    )
    .unwrap();

    // --------------------------------------------------------------------------------
    // Track accounts in the client.
    //
    // Not the same as adding the keys to the authenticator. A client can track accounts without having their signing keys.
    // --------------------------------------------------------------------------------
    faucet_client
        .add_account(&faucet_account_a, Some(faucet_a_seed), false)
        .await
        .unwrap();

    faucet_client
        .add_account(&faucet_account_b, Some(faucet_b_seed), false)
        .await
        .unwrap();

    alice_client
        .add_account(&alice, Some(alice_seed), false)
        .await
        .unwrap();

    bob_client
        .add_account(&bob, Some(bob_seed), false)
        .await
        .unwrap();

    matcher_client
        .add_account(&matcher, Some(matcher_seed), false)
        .await
        .unwrap();

    // --------------------------------------------------------------------------------
    // Mint assets from the faucet accounts for alice and bob
    // --------------------------------------------------------------------------------
    println!("Minting assets for Alice and Bob...");
    let mint_asset_a: FungibleAsset = FungibleAsset::new(faucet_account_a.id(), 100).unwrap();
    // mint assets B for Bob
    let mint_asset_b = FungibleAsset::new(faucet_account_b.id(), 200).unwrap();

    let transaction_request_a = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            mint_asset_a,
            alice.id(),
            NoteType::Public,
            faucet_client.rng(),
        )
        .unwrap();

    let transaction_request_b = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            mint_asset_b,
            bob.id(),
            NoteType::Public,
            faucet_client.rng(),
        )
        .unwrap();

    let tx_result_a = faucet_client
        .new_transaction(faucet_account_a.id(), transaction_request_a)
        .await
        .unwrap();
    let note_for_alice = tx_result_a.created_notes().iter().next().unwrap();

    faucet_client
        .submit_transaction(tx_result_a.clone())
        .await
        .unwrap();
    println!("Submitted mint transaction for Alice");

    let tx_result_b = faucet_client
        .new_transaction(faucet_account_b.id(), transaction_request_b)
        .await
        .unwrap();

    let note_for_bob = tx_result_b.created_notes().iter().next().unwrap();

    faucet_client
        .submit_transaction(tx_result_b.clone())
        .await
        .unwrap();
    println!("Submitted mint transaction for Bob");

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
                println!("Note found on chain, breaking");
                break;
            }
            Err(ClientError::NoteNotFoundOnChain(_)) => {
                // Wait for 1 second before trying again
                println!("Note not found on chain, waiting for 1 second before retrying");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                // Sync state again before retrying
                alice_client.sync_state().await.unwrap();
            }
            _ => {
                panic!("Failed");
            }
        }
    }

    // same for bob
    println!("Waiting for Bob's note to be confirmed on chain...");
    let start_time = std::time::Instant::now();
    let mut notes_for_bob = Vec::new();
    while start_time.elapsed() < timeout {
        let note_file = NoteFile::NoteId(note_for_bob.id());
        match bob_client.import_note(note_file).await {
            Ok(note) => {
                notes_for_bob.push(note);
                bob_client.sync_state().await.unwrap();
                println!("Note found on chain, breaking");
                break;
            }
            Err(ClientError::NoteNotFoundOnChain(_)) => {
                // Wait for 1 second before trying again
                println!("Note not found on chain, waiting for 1 second before retrying");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                // Sync state again before retrying
                bob_client.sync_state().await.unwrap();
            }
            _ => {
                panic!("Failed");
            }
        }
    }

    // Only panic if we've timed out without finding the note
    if notes_for_alice.is_empty() || notes_for_bob.is_empty() {
        panic!("Notes not found on chain after 10 seconds");
    }

    // Need to have Alice and Bob consume the notes created by the faucet accounts.
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

    println!("Building consume transaction for Bob...");
    let consume_request_b = TransactionRequestBuilder::new()
        .build_consume_notes(notes_for_bob)
        .unwrap();

    let tx_result_b = bob_client
        .new_transaction(bob.id(), consume_request_b)
        .await
        .unwrap();

    bob_client.submit_transaction(tx_result_b).await.unwrap();
    println!("Submitted consume transaction for Bob");

    // --------------------------------------------------------------------------------
    // Now create swap requests by alice and bob
    // --------------------------------------------------------------------------------
    let swap_data_a = SwapTransactionData::new(
        alice.id(),
        Asset::Fungible(FungibleAsset::new(faucet_account_a.id(), 10).unwrap()),
        Asset::Fungible(FungibleAsset::new(faucet_account_b.id(), 20).unwrap()),
    );

    let swap_request_a = TransactionRequestBuilder::new()
        .in_flight_swap(&swap_data_a)
        .unwrap();

    let swap_data_b = SwapTransactionData::new(
        bob.id(),
        Asset::Fungible(FungibleAsset::new(faucet_account_b.id(), 20).unwrap()),
        Asset::Fungible(FungibleAsset::new(faucet_account_a.id(), 10).unwrap()),
    );

    let swap_request_b = TransactionRequestBuilder::new()
        .in_flight_swap(&swap_data_b)
        .unwrap();

    let tx_result_a = alice_client
        .new_transaction(alice.id(), swap_request_a.clone())
        .await
        .unwrap();

    let tx_result_b = bob_client
        .new_transaction(bob.id(), swap_request_b.clone())
        .await
        .unwrap();

    // TODO currently miden-client exposes `testing_prove_transaction` but only under the `testing` feature flag. Note to self to PR upstream to expose this, as well as `testing_submit_proven_transaction` under `pub` visibility.

    // now we don't actually want to submit the tx right away to the network, but rather to the CLOB aggregator. We only prove the tx here.
    println!("Proving Alice's swap transaction...");
    let proven_tx_a = alice_client
        .testing_prove_transaction(&tx_result_a)
        .await
        .unwrap();
    println!("Alice's swap transaction proven");

    // At this point we can submit the proven transaction to the CLOB aggregator.
    // TODO: Implement this.

    println!("Proving Bob's swap transaction...");
    let proven_tx_b = bob_client
        .testing_prove_transaction(&tx_result_b)
        .await
        .unwrap();
    println!("Bob's swap transaction proven");

    // Also submit Bob's
    // TODO: Implement this.

    // --------------------------------------------------------------------------------
    // Now assume a matcher has queried the CLOB aggregator and found a match.
    // The matcher will then submit the tx to the network, which actually submits Alice's and Bob's txs and gets them included in a block. Then, the matcher will consume the two notes and output new notes for Alice and Bob.
    // --------------------------------------------------------------------------------

    let swap_request_output_notes_a = proven_tx_a.output_notes().iter().next().unwrap();
    let swap_request_output_notes_b = proven_tx_b.output_notes().iter().next().unwrap();

    let mut consume_swap_input_notes = Vec::new();

    // This is where serialization would happen.
    if let OutputNote::Full(note) = swap_request_output_notes_a {
        consume_swap_input_notes.push(note.clone());
    } else {
        panic!("Note type is not Full");
    }

    if let OutputNote::Full(note) = swap_request_output_notes_b {
        consume_swap_input_notes.push(note.clone());
    } else {
        panic!("Note type is not Full");
    }

    println!("Building matcher's consume swap transaction...");

    let consume_swap_request = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes(
            consume_swap_input_notes
                .into_iter()
                .map(|note| (note, None))
                .collect::<Vec<_>>(),
        )
        .build()
        .unwrap();

    let consume_swap_tx_result = matcher_client
        .new_transaction(matcher.id(), consume_swap_request)
        .await
        .unwrap();

    // --------------------------------------------------------------------------------
    // The matcher will submit all the txs to the network.
    // --------------------------------------------------------------------------------
    println!("Matcher submitting Alice's proven transaction to the network...");
    matcher_client
        .testing_submit_proven_transaction(proven_tx_a.clone())
        .await
        .unwrap();
    println!("Matcher submitted Alice's proven transaction");

    println!("Matcher submitting Bob's proven transaction to the network...");
    matcher_client
        .testing_submit_proven_transaction(proven_tx_b.clone())
        .await
        .unwrap();
    println!("Matcher submitted Bob's proven transaction");

    println!("Submitting matcher's consume swap transaction...");
    matcher_client
        .submit_transaction(consume_swap_tx_result)
        .await
        .unwrap();
    println!("Matcher's consume swap transaction submitted");

    // --------------------------------------------------------------------------------
    // Now Alice and Bob can submit their txs to the network to consume the output notes.
    // --------------------------------------------------------------------------------
}
