// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::{
    base_node::{
        base_node::BaseNodeStateMachine,
        states::{InitialSync, ListeningInfo, StateEvent},
    },
    blocks::BlockHash,
    chain_storage::{async_db, BlockchainBackend, ChainMetadata, ChainStorageError},
};
use log::*;
use std::collections::VecDeque;
use tari_crypto::tari_utilities::{hex::Hex, Hashable};

const LOG_TARGET: &str = "c::bn::states::block_sync";

// The maximum number of retry attempts a node can perform to request a particular block from remote nodes.
const MAX_HEADER_REQUEST_RETRY_ATTEMPTS: usize = 5;
const MAX_BLOCK_REQUEST_RETRY_ATTEMPTS: usize = 5;

/// Configuration for the Block Synchronization.
#[derive(Clone, Copy)]
pub struct BlockSyncConfig {
    pub max_header_request_retry_attempts: usize,
    pub max_block_request_retry_attempts: usize,
}

impl Default for BlockSyncConfig {
    fn default() -> Self {
        Self {
            max_header_request_retry_attempts: MAX_HEADER_REQUEST_RETRY_ATTEMPTS,
            max_block_request_retry_attempts: MAX_BLOCK_REQUEST_RETRY_ATTEMPTS,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockSyncInfo;

impl BlockSyncInfo {
    pub async fn next_event<B: BlockchainBackend + 'static>(
        &mut self,
        shared: &mut BaseNodeStateMachine<B>,
    ) -> StateEvent
    {
        info!(target: LOG_TARGET, "Synchronizing missing blocks");

        match synchronize_blocks(shared).await {
            Ok(StateEvent::BlocksSynchronized) => {
                info!(target: LOG_TARGET, "Block sync state has synchronised");
                StateEvent::BlocksSynchronized
            },
            Ok(StateEvent::MaxRequestAttemptsReached) => {
                warn!(
                    target: LOG_TARGET,
                    "Maximum unsuccessful header/block request attempts reached"
                );
                StateEvent::MaxRequestAttemptsReached
            },
            Ok(state_event) => state_event,
            Err(e) => StateEvent::FatalError(format!("Synchronizing blocks failed. {}", e)),
        }
    }
}

/// State management for Listening -> BlockSync. This change happens when a node has been temporarily disconnected
/// from the network, or a reorg has occurred.
impl From<ListeningInfo> for BlockSyncInfo {
    fn from(_old: ListeningInfo) -> Self {
        BlockSyncInfo {}
    }
}

/// State management for InitialSync -> BlockSync. This change happens when a (previously synced) node is restarted
/// after being offline for some time.
impl From<InitialSync> for BlockSyncInfo {
    fn from(_old: InitialSync) -> Self {
        BlockSyncInfo {}
    }
}

async fn network_tip_metadata<B: BlockchainBackend>(
    shared: &mut BaseNodeStateMachine<B>,
) -> Result<ChainMetadata, String> {
    let metadata_list = shared.comms.get_metadata().await.map_err(|e| e.to_string())?;
    // TODO: Use heuristics to weed out outliers / dishonest nodes.
    Ok(metadata_list
        .into_iter()
        .fold(ChainMetadata::default(), |best, current| {
            if current.accumulated_difficulty.unwrap_or(0.into()) >= best.accumulated_difficulty.unwrap_or(0.into()) {
                current
            } else {
                best
            }
        }))
}

async fn synchronize_blocks<B: BlockchainBackend + 'static>(
    shared: &mut BaseNodeStateMachine<B>,
) -> Result<StateEvent, String> {
    let local_metadata = shared.db.get_metadata().map_err(|e| e.to_string())?;
    let network_metadata = network_tip_metadata(shared).await?;

    if let Some(mut sync_block_hash) = network_metadata.best_block {
        // Find the missing block hashes of the strongest network chain.
        let mut attempts: usize = 0;
        let mut block_hashes = VecDeque::<BlockHash>::new();
        let mut linked_to_chain = false;
        while local_metadata.accumulated_difficulty.unwrap_or(0.into()) <
            network_metadata.accumulated_difficulty.unwrap_or(0.into())
        {
            // Check if sync hash is on local chain.
            if async_db::fetch_header_with_block_hash(shared.db.clone(), sync_block_hash.clone())
                .await
                .is_ok()
            {
                linked_to_chain = true;
                break;
            }
            // Check if blockchain db already has the sync hash block.
            if let Ok(block) = async_db::fetch_orphan(shared.db.clone(), sync_block_hash.clone()).await {
                sync_block_hash = block.header.prev_hash;
                continue;
            }
            // Add missing block to download queue.
            block_hashes.push_front(sync_block_hash.clone());
            // Find the previous block hash by requesting the current header from a random peer node.
            match shared
                .comms
                .fetch_headers_with_hashes(vec![sync_block_hash.clone()])
                .await
            {
                Ok(headers) => {
                    debug!(target: LOG_TARGET, "Received {} headers from peer", headers.len());
                    if let Some(header) = headers.first() {
                        if header.hash() == sync_block_hash {
                            attempts = 0;
                            sync_block_hash = header.prev_hash.clone();
                            continue;
                        }
                    }
                },
                Err(e) => {
                    warn!(
                        target: LOG_TARGET,
                        "Failed to fetch header from peer:{:?}. Retrying.", e,
                    );
                },
            }
            // Attempt again to retrieve the correct header.
            attempts += 1;
            if attempts >= shared.config.block_sync_config.max_header_request_retry_attempts {
                return Ok(StateEvent::MaxRequestAttemptsReached);
            }
        }

        // Sync missing blocks
        if linked_to_chain {
            for sync_block_hash in block_hashes {
                attempts = 0;
                while attempts < shared.config.block_sync_config.max_block_request_retry_attempts {
                    // Request the block from a random peer node and add to chain.
                    match shared
                        .comms
                        .fetch_blocks_with_hashes(vec![sync_block_hash.clone()])
                        .await
                    {
                        Ok(blocks) => {
                            debug!(target: LOG_TARGET, "Received {} blocks from peer", blocks.len());
                            if let Some(hist_block) = blocks.first() {
                                let block_hash = hist_block.block().hash();
                                if block_hash == sync_block_hash {
                                    match shared.db.add_block(hist_block.block().clone()) {
                                        Ok(_) => {
                                            break;
                                        },
                                        Err(ChainStorageError::InvalidBlock) => {
                                            warn!(
                                                target: LOG_TARGET,
                                                "Invalid block {} received from peer. Retrying",
                                                block_hash.to_hex(),
                                            );
                                        },
                                        Err(ChainStorageError::ValidationError(_)) => {
                                            warn!(
                                                target: LOG_TARGET,
                                                "Validation on block {} from peer failed. Retrying",
                                                block_hash.to_hex(),
                                            );
                                        },
                                        Err(e) => return Err(e.to_string()),
                                    }
                                }
                            }
                        },
                        Err(e) => {
                            warn!(
                                target: LOG_TARGET,
                                "Failed to fetch blocks from peer:{:?}. Retrying.", e,
                            );
                        },
                    }
                    // Attempt again to retrieve the correct block
                    attempts += 1;
                }
                if attempts >= shared.config.block_sync_config.max_block_request_retry_attempts {
                    return Ok(StateEvent::MaxRequestAttemptsReached);
                }
            }
        } else {
            warn!(target: LOG_TARGET, "Network fork chain not linked to local chain.",);
        }
    }

    Ok(StateEvent::BlocksSynchronized)
}
