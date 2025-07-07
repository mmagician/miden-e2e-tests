use miden_client::{
    ExecutionOptions, Word,
    crypto::FeltRng,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    transaction::{OutputNote, TransactionRequestBuilder},
};
use miden_lib::{note::utils::build_p2id_recipient, transaction::TransactionKernel};
use miden_objects::{Felt, account::AccountId, asset::Asset};
use miden_tx::utils::word_to_masm_push_string;

use {
    miden_client::{
        Client,
        crypto::RpoRandomCoin,
        rpc::{Endpoint, TonicRpcClient},
        store::sqlite_store::SqliteStore,
    },
    miden_tx::auth::TransactionAuthenticator,
    std::{fs, path::Path, sync::Arc},
};

pub trait DrainFaucet {
    fn drain_faucet(
        &self,
        receiver_id: AccountId,
        asset_to_burn: Asset,
    ) -> TransactionRequestBuilder;
}

impl DrainFaucet for TransactionRequestBuilder {
    fn drain_faucet(
        &self,
        receiver_id: AccountId,
        asset_to_burn: Asset,
    ) -> TransactionRequestBuilder {
        let note = get_faucet_drain_note(receiver_id, asset_to_burn);

        Self::new().with_own_output_notes(vec![OutputNote::Full(note)])
    }
}

fn get_faucet_drain_note(receiver_id: AccountId, asset_to_burn: Asset) -> Note {
    let mut rng = RpoRandomCoin::new([Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)]);

    let recipient = build_p2id_recipient(receiver_id, Word::default()).unwrap();

    let note_type = NoteType::Public;
    let note_execution_hint = NoteExecutionHint::Always;
    let aux = Felt::new(27);
    let tag = NoteTag::from_account_id(receiver_id);
    let amount = Felt::new(250);

    println!(
        "recipient_digest: {:?}",
        word_to_masm_push_string(&recipient.digest())
    );

    let note_script = format!(
        "
        # burn the asset
        begin
            dropw

            # pad the stack before call
            padw padw padw padw
            # => [pad(16)]

            exec.::miden::note::get_assets drop
            mem_loadw
            # => [ASSET, pad(12)]
            call.::miden::contracts::faucets::basic_fungible::burn
            dropw dropw dropw dropw

            push.{recipient}
            push.{note_execution_hint}
            push.{note_type}
            push.{aux}
            push.{tag}
            push.{amount}
            # => [amount, tag, aux, note_type, execution_hint, RECIPIENT, pad(7)]

            call.::miden::contracts::faucets::basic_fungible::distribute
            # => [note_idx, pad(15)]

            # truncate the stack
            dropw dropw dropw dropw
        end",
        note_type = note_type as u8,
        recipient = word_to_masm_push_string(&recipient.digest()),
        note_execution_hint = Felt::from(note_execution_hint),
    );

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_script, assembler).unwrap();
    let serial_num = rng.draw_word();

    let faucet_recipient =
        NoteRecipient::new(serial_num, note_script, NoteInputs::new(vec![]).unwrap());

    let note = Note::new(
        NoteAssets::new(vec![asset_to_burn]).unwrap(),
        NoteMetadata::new(
            receiver_id,
            NoteType::Public,
            NoteTag::for_public_use_case(123, 0, NoteExecutionMode::Local).unwrap(),
            NoteExecutionHint::Always,
            Felt::new(0),
        )
        .unwrap(),
        faucet_recipient,
    );
    note
}

/// Removes the test SQLite store file if it exists.
pub async fn reset_store() {
    let db_files = [
        "store.sqlite3",
        "faucet_store.sqlite3",
        "alice_store.sqlite3",
        "bob_store.sqlite3",
        "matcher_store.sqlite3",
    ];

    for filename in &db_files {
        let path = Path::new(filename);
        if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }
}

pub async fn setup_client<T: TransactionAuthenticator + 'static>(
    authenticator: Arc<T>,
    db_filename: &str,
) -> Result<Client, Box<dyn std::error::Error>> {
    let sqlite_store = SqliteStore::new(db_filename.into()).await?;
    let store = Arc::new(sqlite_store);

    let rng = RpoRandomCoin::new(Default::default());

    let endpoint = Endpoint::localhost();

    let mut client = Client::new(
        Arc::new(TonicRpcClient::new(&endpoint, 100)),
        Box::new(rng),
        store,
        authenticator,
        ExecutionOptions::default(),
        None,
        None,
    );

    let _ = client.sync_state().await;

    Ok(client)
}
