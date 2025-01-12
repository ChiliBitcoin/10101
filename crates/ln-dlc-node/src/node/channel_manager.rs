use crate::dlc_custom_signer::CustomKeysManager;
use crate::fee_rate_estimator::FeeRateEstimator;
use crate::ln::TracingLogger;
use crate::ln_dlc_wallet::LnDlcWallet;
use crate::node::Storage;
use crate::storage::TenTenOneStorage;
use crate::ChainMonitor;
use crate::Router;
use anyhow::anyhow;
use anyhow::Result;
use bitcoin::BlockHash;
use lightning::chain::BestBlock;
use lightning::chain::ChannelMonitorUpdateStatus;
use lightning::chain::Watch;
use lightning::ln::channelmanager::ChainParameters;
use lightning::ln::channelmanager::ChannelManagerReadArgs;
use lightning::util::config::UserConfig;
use lightning::util::persist::read_channel_monitors;
use lightning::util::persist::KVStore;
use lightning::util::persist::CHANNEL_MANAGER_PERSISTENCE_KEY;
use lightning::util::persist::CHANNEL_MANAGER_PERSISTENCE_PRIMARY_NAMESPACE;
use lightning::util::persist::CHANNEL_MANAGER_PERSISTENCE_SECONDARY_NAMESPACE;
use lightning::util::ser::ReadableArgs;
use lightning_transaction_sync::EsploraSyncClient;
use std::sync::Arc;

pub type ChannelManager<S, N> = lightning::ln::channelmanager::ChannelManager<
    Arc<ChainMonitor<S, N>>,
    Arc<LnDlcWallet<S, N>>,
    Arc<CustomKeysManager<S, N>>,
    Arc<CustomKeysManager<S, N>>,
    Arc<CustomKeysManager<S, N>>,
    Arc<FeeRateEstimator>,
    Arc<Router>,
    Arc<TracingLogger>,
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn build<S: TenTenOneStorage, N: Storage>(
    keys_manager: Arc<CustomKeysManager<S, N>>,
    ln_dlc_wallet: Arc<LnDlcWallet<S, N>>,
    fee_rate_estimator: Arc<FeeRateEstimator>,
    explora_client: Arc<EsploraSyncClient<Arc<TracingLogger>>>,
    logger: Arc<TracingLogger>,
    chain_monitor: Arc<ChainMonitor<S, N>>,
    ldk_config: UserConfig,
    network: bitcoin::Network,
    persister: Arc<S>,
    router: Arc<Router>,
) -> Result<ChannelManager<S, N>> {
    let file = match KVStore::read(
        persister.as_ref(),
        CHANNEL_MANAGER_PERSISTENCE_PRIMARY_NAMESPACE,
        CHANNEL_MANAGER_PERSISTENCE_SECONDARY_NAMESPACE,
        CHANNEL_MANAGER_PERSISTENCE_KEY,
    ) {
        Ok(manager) => {
            tracing::info!("Found channel manager data. Recovering from stored state");
            manager
        }
        Err(e) => {
            tracing::info!("Did not find channel manager data. {e:#}");
            tracing::info!("Initializing new channel manager");

            let (height, block_hash) = ln_dlc_wallet.tip()?;
            return Ok(ChannelManager::new(
                fee_rate_estimator,
                chain_monitor.clone(),
                ln_dlc_wallet,
                router,
                logger,
                keys_manager.clone(),
                keys_manager.clone(),
                keys_manager,
                ldk_config,
                ChainParameters {
                    network,
                    best_block: BestBlock::new(block_hash, height),
                },
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as u32,
            ));
        }
    };

    let mut channelmonitors =
        read_channel_monitors(persister, keys_manager.clone(), keys_manager.clone())?;

    let mut channel_monitor_mut_references = Vec::new();
    for (_, channel_monitor) in channelmonitors.iter_mut() {
        channel_monitor_mut_references.push(channel_monitor);
    }
    let read_args = ChannelManagerReadArgs::new(
        keys_manager.clone(),
        keys_manager.clone(),
        keys_manager,
        fee_rate_estimator,
        chain_monitor.clone(),
        ln_dlc_wallet,
        router,
        logger,
        ldk_config,
        channel_monitor_mut_references,
    );

    let channel_manager =
        <(BlockHash, ChannelManager<S, N>)>::read(&mut file.as_slice(), read_args)
            .map_err(|e| anyhow!(e))?
            .1;

    // Make sure our filter is initialized with all the txs and outputs
    // that we need to be watching based on our set of channel monitors
    for (_, monitor) in channelmonitors.iter() {
        monitor.load_outputs_to_watch(&explora_client.clone());
    }

    for (_, monitor) in channelmonitors.drain(..) {
        // ATTENTION: This must be `get_original_funding_txo` and _not_ `get_funding_txo`, because
        // we are using LN-DLC channels. `rust-dlc` is manipulating the funding TXO so that LDK
        // considers the `glue_transaction` as the `funding_transaction` for certain purposes.
        //
        // For other purposes, LDK must still refer back to the original `funding_transaction`. This
        // is one such case.
        let funding_txo = monitor.get_original_funding_txo().0;
        let channel_monitor_update_status = chain_monitor
            .watch_channel(funding_txo, monitor)
            .map_err(|_| anyhow!("Failed to watch channel. funding_txo={funding_txo:?}"))?;
        assert_eq!(
            channel_monitor_update_status,
            ChannelMonitorUpdateStatus::Completed
        );
    }

    Ok(channel_manager)
}
