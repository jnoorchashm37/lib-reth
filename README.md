# Reth Ethereum Library
A library for connecting to and interacting with various ethereum endpoints offered by [reth](https://github.com/paradigmxyz/reth)

### Connection Types
- http (default)
- ipc (feature = `ipc`)
- ws (feature = `ws`)
- libmdbx-db (feature = `libmdbx`)

### Streaming
The `ipc`, `ws`, and `libmdbx` features implement the `EthStream` trait which contains functionality for creating streams for:
- Blocks
- Pending full txs
- Pending tx hashes
- Logs