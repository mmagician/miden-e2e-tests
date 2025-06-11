use miden_client::{
    Word,
    crypto::FeltRng,
    note::{
        Note, NoteAssets, NoteError, NoteExecutionHint, NoteExecutionMode, NoteInputs,
        NoteMetadata, NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    transaction::{
        OutputNote, SwapTransactionData, TransactionRequest, TransactionRequestBuilder,
        TransactionRequestError,
    },
};
use miden_lib::{
    note::utils::{build_p2id_recipient, build_swap_tag},
    transaction::TransactionKernel,
};
use miden_objects::{Felt, FieldElement, account::AccountId, asset::Asset, note::NoteDetails};

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

/// This is a modification of `create_swap_note` to create an in-flight swap note. The consumer of
/// this note does not receive the `offered_asset` directly, and only acts as an intermediary. The
/// consumer will create a new P2ID note with `sender` as target, containing the `requested_asset`.
fn create_in_flight_swap_note(
    sender_account_id: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
) -> (Note, NoteDetails) {
    let mut rng = RpoRandomCoin::new([Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)]);
    let note_type = NoteType::Public;
    let aux = Felt::new(0);

    let note_script = NoteScript::compile(
        IN_FLIGHT_SWAP_NOTE_SCRIPT,
        TransactionKernel::testing_assembler(),
    )
    .unwrap();

    let payback_serial_num = rng.draw_word();
    let payback_recipient = build_p2id_recipient(sender_account_id, payback_serial_num).unwrap();

    let payback_recipient_word: Word = payback_recipient.digest().into();
    let requested_asset_word: Word = requested_asset.into();
    let payback_tag =
        NoteTag::from_account_id(sender_account_id, NoteExecutionMode::Local).unwrap();

    let inputs = NoteInputs::new(vec![
        payback_recipient_word[0],
        payback_recipient_word[1],
        payback_recipient_word[2],
        payback_recipient_word[3],
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        payback_tag.into(),
        NoteExecutionHint::always().into(),
    ])
    .unwrap();

    let tag = build_swap_tag(note_type, &offered_asset, &requested_asset).unwrap();
    let serial_num = rng.draw_word();

    let metadata = NoteMetadata::new(
        sender_account_id,
        note_type,
        tag,
        NoteExecutionHint::always(),
        aux,
    )
    .unwrap();
    let assets = NoteAssets::new(vec![offered_asset]).unwrap();
    let recipient = NoteRecipient::new(serial_num, note_script, inputs);
    let note = Note::new(assets, metadata, recipient);

    let payback_assets = NoteAssets::new(vec![requested_asset]).unwrap();
    let payback_note = NoteDetails::new(payback_assets, payback_recipient);

    (note, payback_note)
}

const IN_FLIGHT_SWAP_NOTE_SCRIPT: &str = r"
use.miden::note
use.miden::tx
use.miden::contracts::wallets::basic
use.miden::contracts::wallets::aux

# CONSTANTS
# =================================================================================================

const.PRIVATE_NOTE=2

#! Swap script:
#! Creates a note consumable by note issuer containing requested ASSET.
#!
#! Requires that the account exposes:
#! - miden::contracts::wallets::basic::create_note procedure.
#! - miden::contracts::wallets::aux::add_asset_to_note procedure.
#!
#! Inputs:  []
#! Outputs: []
#!
#! Note inputs are assumed to be as follows:
#! - RECIPIENT
#! - ASSET
#! - TAG = [tag, 0, 0, 0]
#!
#! Panics if:
#! - account does not expose miden::contracts::wallets::basic::create_note procedure.
#! - account does not expose miden::contracts::wallets::aux::add_asset_to_note procedure.
begin
    # store note inputs into memory starting at address 0
    push.0 exec.note::get_inputs
    # => [num_inputs, inputs_ptr]

    # make sure the number of inputs is 10
    eq.10 assert
    # => [inputs_ptr]

    # load RECIPIENT
    drop padw mem_loadw
    # => [RECIPIENT]

    padw mem_loadw.4
    # => [ASSET, RECIPIENT]

    padw mem_loadw.8
    # => [0, 0, execution_hint, tag, ASSET, RECIPIENT]

    drop drop swap
    # => [tag, execution_hint, ASSET, RECIPIENT]

    # aux = 0, not used
    push.0 swap
    # => [tag, aux, execution_hint, ASSET, RECIPIENT]

    push.PRIVATE_NOTE movdn.2
    # => [tag, aux, note_type, execution_hint, ASSET, RECIPIENT]

    swapw
    # => [ASSET, tag, aux, note_type, execution_hint, RECIPIENT]

    # create a note using inputs
    padw swapdw padw movdnw.2
    # => [tag, aux, note_type, execution_hint, RECIPIENT, pad(8), ASSET]
    call.basic::create_note
    # => [note_idx, pad(15), ASSET]

    swapw dropw movupw.3
    # => [ASSET, note_idx, pad(11)]

    # move asset to the note
    call.aux::add_asset_to_note
    # => [ASSET, note_idx, pad(11)]

    # clean stack
    dropw dropw dropw dropw
    # => []
end
";

pub trait InFlightSwap {
    /// Create a new in-flight swap transaction request from `SwapTransactionData`.
    fn in_flight_swap(
        &self,
        data: &SwapTransactionData,
    ) -> Result<TransactionRequest, TransactionRequestError>;
}

impl InFlightSwap for TransactionRequestBuilder {
    fn in_flight_swap(
        &self,
        data: &SwapTransactionData,
    ) -> Result<TransactionRequest, TransactionRequestError> {
        let (created_note, payback_note_details) = create_in_flight_swap_note(
            data.account_id(),
            data.offered_asset(),
            data.requested_asset(),
        );

        let payback_tag = NoteTag::from_account_id(data.account_id(), NoteExecutionMode::Local)?;

        Self::new()
            .with_expected_future_notes(vec![(payback_note_details, payback_tag)])
            .with_own_output_notes(vec![OutputNote::Full(created_note)])
            .build()
    }
}

/// Removes the test SQLite store file if it exists.
pub async fn reset_store() {
    let filename = Path::new("store.sqlite3");
    if filename.exists() {
        fs::remove_file(filename).unwrap();
    }
}

pub async fn setup_client<T: TransactionAuthenticator + 'static>(
    authenticator: Arc<T>,
) -> Result<Client, Box<dyn std::error::Error>> {
    let sqlite_store = SqliteStore::new("store.sqlite3".into()).await?;
    let store = Arc::new(sqlite_store);

    let rng = RpoRandomCoin::new(Default::default());

    let endpoint = Endpoint::localhost();

    let mut client = Client::new(
        Arc::new(TonicRpcClient::new(&endpoint, 100)),
        Box::new(rng),
        store,
        authenticator,
        false,
        None,
        None,
    );

    let _ = client.sync_state().await;

    Ok(client)
}
