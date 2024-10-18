use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};
use reth_provider::{ProviderError, StateProvider};
use reth_revm::database::StateProviderDatabase;
use revm::{
    bytecode::{
        eip7702::Eip7702Bytecode,
        eof::{EofBody, EofHeader, TypesSection},
        Eof, JumpTable, LegacyAnalyzedBytecode, LegacyRawBytecode
    },
    state::{AccountInfo, Bytecode},
    DatabaseRef
};

pub struct RethLibmdbxDatabaseRef(StateProviderDatabase<Box<dyn StateProvider>>);

impl RethLibmdbxDatabaseRef {
    pub fn new(this: StateProviderDatabase<Box<dyn StateProvider>>) -> Self {
        Self(this)
    }
}

impl DatabaseRef for RethLibmdbxDatabaseRef {
    type Error = ProviderError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(reth_revm::DatabaseRef::basic_ref(&self.0, address)?.map(|acct| AccountInfo {
            balance:   acct.balance,
            nonce:     acct.nonce,
            code_hash: acct.code_hash,
            code:      acct.code.map(change_bytecode)
        }))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(change_bytecode(reth_revm::DatabaseRef::code_by_hash_ref(&self.0, code_hash)?))
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(reth_revm::DatabaseRef::storage_ref(&self.0, address, index)?)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        Ok(reth_revm::DatabaseRef::block_hash_ref(&self.0, number)?)
    }
}

fn change_bytecode(bytes: reth_revm::primitives::Bytecode) -> Bytecode {
    match bytes {
        reth_revm::primitives::Bytecode::LegacyRaw(bytes) => Bytecode::LegacyRaw(LegacyRawBytecode(bytes)),
        reth_revm::primitives::Bytecode::LegacyAnalyzed(legacy_analyzed_bytecode) => {
            Bytecode::LegacyAnalyzed(LegacyAnalyzedBytecode::new(
                legacy_analyzed_bytecode.bytecode().clone(),
                legacy_analyzed_bytecode.original_len().clone(),
                JumpTable(legacy_analyzed_bytecode.jump_table().clone().0)
            ))
        }
        reth_revm::primitives::Bytecode::Eof(arc) => Bytecode::Eof(change_eof(arc)),
        reth_revm::primitives::Bytecode::Eip7702(eip7702_bytecode) => Bytecode::Eip7702(Eip7702Bytecode {
            delegated_address: eip7702_bytecode.delegated_address,
            version:           eip7702_bytecode.version,
            raw:               eip7702_bytecode.raw
        })
    }
}

fn change_eof(eof: Arc<reth_revm::primitives::Eof>) -> Arc<Eof> {
    Arc::new(Eof {
        header: EofHeader {
            types_size:          eof.header.types_size,
            code_sizes:          eof.header.code_sizes.clone(),
            container_sizes:     eof.header.container_sizes.clone(),
            data_size:           eof.header.data_size,
            sum_code_sizes:      eof.header.sum_code_sizes,
            sum_container_sizes: eof.header.sum_container_sizes
        },
        body:   EofBody {
            types_section:     eof
                .body
                .types_section
                .clone()
                .into_iter()
                .map(|sec| TypesSection {
                    inputs:         sec.inputs,
                    outputs:        sec.outputs,
                    max_stack_size: sec.max_stack_size
                })
                .collect(),
            code_section:      eof.body.code_section.clone(),
            container_section: eof.body.container_section.clone(),
            data_section:      eof.body.data_section.clone(),
            is_data_filled:    eof.body.is_data_filled
        },
        raw:    eof.raw.clone()
    })
}
