use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};
use reth_provider::StateProvider;
use reth_revm::database::StateProviderDatabase;
use revm::{
    bytecode::{eip7702::Eip7702Bytecode, Eof, JumpTable, LegacyAnalyzedBytecode},
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

fn change_bytecode(bytes: reth_revm::primitives::Bytecode) -> eyre::Result<Bytecode> {
    let new_bytecode = match bytes {
        reth_revm::primitives::Bytecode::LegacyRaw(bytes) => Bytecode::new_legacy(bytes),
        reth_revm::primitives::Bytecode::LegacyAnalyzed(legacy_analyzed_bytecode) => {
            Bytecode::LegacyAnalyzed(LegacyAnalyzedBytecode::new(
                legacy_analyzed_bytecode.bytecode().clone(),
                legacy_analyzed_bytecode.original_len(),
                JumpTable(legacy_analyzed_bytecode.jump_table().clone().0),
            ))
        }
        reth_revm::primitives::Bytecode::Eof(arc) => Bytecode::Eof(change_eof(arc)?),
        reth_revm::primitives::Bytecode::Eip7702(eip7702_bytecode) => Bytecode::Eip7702(Eip7702Bytecode {
            delegated_address: eip7702_bytecode.delegated_address,
            version: eip7702_bytecode.version,
            raw: eip7702_bytecode.raw,
        }),
    };

    Ok(new_bytecode)
}

fn change_eof(eof: Arc<reth_revm::primitives::Eof>) -> eyre::Result<Arc<Eof>> {
    Ok(Arc::new(Eof::decode(eof.encode_slow())?))
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

#[cfg(test)]
mod tests {
    use alloy_primitives::bytes;

    use super::*;

    #[test]
    fn test_eof_conversion() {
        let eof = reth_revm::primitives::Eof::decode(bytes!("ef000101000402000100010400000000800000fe")).unwrap();
        let new_eof = change_eof(Arc::new(eof.clone())).unwrap();
        let original_eof = reth_revm::primitives::Eof::decode(new_eof.encode_slow()).unwrap();

        assert_eq!(eof, original_eof);
    }
}
