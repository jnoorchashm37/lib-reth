use alloy_primitives::{Address, B256, U256};
use reth_provider::StateProvider;
use reth_revm::database::StateProviderDatabase;
use revm::{
    bytecode::{eip7702::Eip7702Bytecode, LegacyAnalyzedBytecode},
    context_interface::DBErrorMarker,
    state::{AccountInfo, Bytecode},
    DatabaseRef,
};

pub struct RethLibmdbxDatabaseRef(StateProviderDatabase<Box<dyn StateProvider>>);

impl RethLibmdbxDatabaseRef {
    pub fn new(this: StateProviderDatabase<Box<dyn StateProvider>>) -> Self {
        Self(this)
    }

    pub fn state_provider_ref(&self) -> &StateProviderDatabase<Box<dyn StateProvider>> {
        &self.0
    }
}

impl DatabaseRef for RethLibmdbxDatabaseRef {
    type Error = RevmUtilError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        reth_revm::DatabaseRef::basic_ref(&self.0, address)
            .map_err(RevmUtilError::as_value)?
            .map(|acct| {
                Ok::<_, Self::Error>(AccountInfo {
                    balance: acct.balance,
                    nonce: acct.nonce,
                    code_hash: acct.code_hash,
                    code: acct.code.map(change_bytecode).transpose()?,
                })
            })
            .transpose()
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(change_bytecode(reth_revm::DatabaseRef::code_by_hash_ref(&self.0, code_hash).map_err(RevmUtilError::as_value)?)?)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        reth_revm::DatabaseRef::storage_ref(&self.0, address, index).map_err(RevmUtilError::as_value)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        reth_revm::DatabaseRef::block_hash_ref(&self.0, number).map_err(RevmUtilError::as_value)
    }
}

fn change_bytecode(bytes: reth_revm::bytecode::Bytecode) -> eyre::Result<Bytecode> {
    // Ok(bytes)
    let new_bytecode = match bytes {
        reth_revm::bytecode::Bytecode::LegacyAnalyzed(legacy_analyzed_bytecode) => {
            Bytecode::LegacyAnalyzed(LegacyAnalyzedBytecode::new(
                legacy_analyzed_bytecode.bytecode().clone(),
                legacy_analyzed_bytecode.original_len(),
                legacy_analyzed_bytecode.jump_table().clone(),
            ))
        }
        reth_revm::bytecode::Bytecode::Eip7702(eip7702_bytecode) => Bytecode::Eip7702(Eip7702Bytecode {
            delegated_address: eip7702_bytecode.delegated_address,
            version: eip7702_bytecode.version,
            raw: eip7702_bytecode.raw,
        }),
    };

    Ok(new_bytecode)
}

#[derive(Debug)]
pub struct RevmUtilError(pub eyre::ErrReport);

impl std::fmt::Display for RevmUtilError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RevmUtilError {}

impl DBErrorMarker for RevmUtilError {}

trait AsValue<T> {
    fn as_value(value: T) -> Self
    where
        Self: Sized;
}

impl<T: ToString> AsValue<T> for RevmUtilError {
    fn as_value(value: T) -> Self {
        RevmUtilError(eyre::eyre!("{}", value.to_string()))
    }
}

impl From<eyre::ErrReport> for RevmUtilError {
    fn from(value: eyre::ErrReport) -> Self {
        Self(value)
    }
}

#[cfg(feature = "uniswap-storage")]
mod _uniswap_storage {
    use alloy_network::Network;
    use alloy_primitives::{Address, StorageKey, StorageValue};
    use alloy_provider::Provider;
    use reth_provider::StateProvider;
    use reth_rpc_eth_api::helpers::EthState;
    use uniswap_storage::StorageSlotFetcher;

    use crate::{
        reth_libmdbx::{NodeClientSpec, RethNodeClient},
        traits::reth_revm_utils::RethLibmdbxDatabaseRef,
        DualRethNodeClient,
    };

    #[async_trait::async_trait]
    impl StorageSlotFetcher for RethLibmdbxDatabaseRef {
        async fn storage_at(&self, address: Address, key: StorageKey, _: Option<u64>) -> eyre::Result<StorageValue> {
            Ok(self.0.storage(address, key)?.unwrap_or_default())
        }
    }

    #[async_trait::async_trait]
    impl<Node, P, N> StorageSlotFetcher for DualRethNodeClient<Node, P, N>
    where
        Node: NodeClientSpec,

        P: Provider<N> + Clone,
        N: Network,
    {
        async fn storage_at(
            &self,
            address: Address,
            key: StorageKey,
            block_number: Option<u64>,
        ) -> eyre::Result<StorageValue> {
            Ok(self
                .node_client()
                .eth_api()
                .storage_at(address, key.into(), block_number.map(Into::into))
                .await?
                .into())
        }
    }

    #[async_trait::async_trait]
    impl<Node> StorageSlotFetcher for RethNodeClient<Node>
    where
        Node: NodeClientSpec,
    {
        async fn storage_at(
            &self,
            address: Address,
            key: StorageKey,
            block_number: Option<u64>,
        ) -> eyre::Result<StorageValue> {
            Ok(self
                .eth_api()
                .storage_at(address, key.into(), block_number.map(Into::into))
                .await?
                .into())
        }
    }
}
