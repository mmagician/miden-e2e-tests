# e2e miden tests

Supposed to replicate https://github.com/0xMiden/miden-base/issues/1331 in an e2e setting.

## Running the tests

Ensure the miden node version 0.9 is running on the default port:
```
miden-node bundled start --data-directory data/ --rpc.url http://0.0.0.0:57291
```

Run the tests:

```bash
cargo test drain_faucet --release -- --nocapture
```


Currently fails with: `error: PublicNoteMissingDetails`:

```
thread 'test_drain_faucet' panicked at tests/drain_faucet.rs:273:10:
called `Result::unwrap()` on an `Err` value: TransactionExecutorError(TransactionProgramExecutionFailed(EventError { label: SourceSpan { source_id: SourceId(4294967295), start: ByteIndex(0), end: ByteIndex(0) }, source_file: None, error: PublicNoteMissingDetails(NoteMetadata { sender: V0(AccountIdV0 { prefix: 8234830435807036192, suffix: 110492255836416 }), note_type: Public, tag: NoteTag(3519676416), aux: 27, execution_hint: Always }, RpoDigest([2155810810489536826, 4545489909651930251, 11088022269034045854, 5790935606882168700])) }))
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test test_drain_faucet ... FAILED

```