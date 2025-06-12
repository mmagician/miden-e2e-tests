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
