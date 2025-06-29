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
