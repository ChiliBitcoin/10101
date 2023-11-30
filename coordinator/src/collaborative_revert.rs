use crate::db;
use crate::db::positions::Position;
use crate::message::OrderbookMessage;
use crate::node::storage::NodeStorage;
use crate::notifications::NotificationKind;
use crate::position;
use crate::storage::CoordinatorTenTenOneStorage;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use bitcoin::hashes::hex::ToHex;
use bitcoin::secp256k1::ecdsa::Signature;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Amount;
use bitcoin::OutPoint;
use bitcoin::Transaction;
use commons::Message;
use diesel::r2d2::ConnectionManager;
use diesel::r2d2::Pool;
use diesel::r2d2::PooledConnection;
use diesel::PgConnection;
use dlc::util::weight_to_fee;
use dlc_manager::subchannel::LNChannelManager;
use dlc_manager::subchannel::LnDlcChannelSigner;
use dlc_manager::subchannel::LnDlcSignerProvider;
use dlc_manager::subchannel::SubChannelState;
use dlc_manager::Storage;
use lightning::ln::channelmanager::ChannelDetails;
use ln_dlc_node::node::Node;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::mpsc;

/// The weight for the collaborative revert transaction. The transaction is expected to have 1 input
/// (the funding TXO) and 2 outputs, one for each party.
///
/// If either party were to _not_ have an output, we would be overestimating the weight of the
/// transaction and would end up paying higher fees than necessary.
const COLLABORATIVE_REVERT_TX_WEIGHT: usize = 672;

/// Propose collaboratively reverting the channel identified by `channel_id`.
///
/// A collaborative revert involves signing a new transaction spending from the funding output
/// directly. This can be used to circumvent bugs related to position and subchannel state.
///
/// This API will only work if LDK still has the [`ChannelDetails`] for the channel. If the
/// [`ChannelDetails`] are unavailable, use `propose_collaborative_revert_without_channel_details`
/// instead.
#[allow(clippy::too_many_arguments)]
pub async fn propose_collaborative_revert(
    node: Arc<Node<CoordinatorTenTenOneStorage, NodeStorage>>,
    pool: Pool<ConnectionManager<PgConnection>>,
    sender: mpsc::Sender<OrderbookMessage>,
    channel_id: [u8; 32],
    settlement_price: Decimal,
    fee_rate_sats_vb: u64,
    funding_txo: OutPoint,
) -> Result<()> {
    let channel_details = node
        .channel_manager
        .get_channel_details(&channel_id)
        .context(
        "Cannot propose collaborative revert without ChannelDetails. Use alternative API instead",
    )?;

    let mut conn = pool.get().context("Could not acquire DB lock")?;

    let channel_id_hex = channel_id.to_hex();

    let subchannels = node
        .list_dlc_channels()
        .context("Could not get list of subchannels")?;

    let subchannel = subchannels
        .iter()
        .find(|c| c.channel_id == channel_id)
        .context("Missing subchannel")?;

    let peer_id = subchannel.counter_party;
    let fund_txo_sat = subchannel.fund_value_satoshis;

    let unspendable_punishment_reserve_sat =
        channel_details.counterparty.unspendable_punishment_reserve;

    let (ln_inbound_liquidity_sat, ln_outbound_liquidity_sat) = {
        let ln_inbound_liquidity_sat =
            Decimal::from(channel_details.inbound_capacity_msat) / Decimal::ONE_THOUSAND;
        let ln_inbound_liquidity_sat = ln_inbound_liquidity_sat
            .to_u64()
            .expect("inbound liquidity to fit into u64");

        let ln_outbound_liquidity_sat =
            Decimal::from(channel_details.outbound_capacity_msat) / Decimal::ONE_THOUSAND;
        let ln_outbound_liquidity_sat = ln_outbound_liquidity_sat
            .to_u64()
            .expect("outbound liquidity to fit into u64");

        (ln_inbound_liquidity_sat, ln_outbound_liquidity_sat)
    };

    let is_channel_split = match channel_details {
        ChannelDetails {
            funding_txo: Some(funding_txo),
            original_funding_outpoint: Some(original_funding_txo),
            ..
        } => {
            // The channel _is_ split if the `original_funding_txo` and the `funding_txo` differ.
            original_funding_txo != funding_txo
        }
        ChannelDetails {
            funding_txo: Some(_),
            original_funding_outpoint: None,
            ..
        } => {
            // The channel is _not_ split if the `original_funding_txo` has not been set.
            false
        }
        ChannelDetails {
            funding_txo: None, ..
        } => {
            bail!("Cannot collaboratively revert channel without funding TXO");
        }
    };

    let coordinator_amount = if is_channel_split {
        let position = Position::get_position_by_trader(&mut conn, peer_id, vec![])?
            .context("Could not load position for channel_id")?;

        // How much the counterparty would get if we were to settle the DLC channel at the
        // `settlement_price` using the standard subchannel collaborative close protocol.
        let counterparty_settlement_amount = position
            .calculate_accept_settlement_amount(settlement_price)
            .context("Could not calculate settlement amount")?;

        let dlc_channel_reserved_tx_fees = estimate_subchannel_reserved_tx_fees(
            fund_txo_sat,
            ln_inbound_liquidity_sat,
            ln_outbound_liquidity_sat,
            unspendable_punishment_reserve_sat,
            position.trader_margin as u64,
            position.coordinator_margin as u64,
        )?;
        let dlc_channel_reserved_tx_fees_per_party = dlc_channel_reserved_tx_fees as f64 / 2.0;

        fund_txo_sat as i64
            - ln_inbound_liquidity_sat as i64
            - unspendable_punishment_reserve_sat as i64
            - counterparty_settlement_amount as i64
            - dlc_channel_reserved_tx_fees_per_party as i64
    } else {
        fund_txo_sat as i64
            - ln_inbound_liquidity_sat as i64
            - unspendable_punishment_reserve_sat as i64
    };

    let trader_amount = subchannel.fund_value_satoshis - coordinator_amount as u64;

    let fee = weight_to_fee(COLLABORATIVE_REVERT_TX_WEIGHT, fee_rate_sats_vb)
        .expect("To be able to calculate constant fee rate");

    let coordinator_address = node.get_unused_address();
    let coordinator_amount = Amount::from_sat(coordinator_amount as u64 - fee / 2);
    let trader_amount = Amount::from_sat(trader_amount - fee / 2);

    tracing::info!(
        channel_id = channel_id_hex,
        coordinator_address = %coordinator_address,
        coordinator_amount = coordinator_amount.to_sat(),
        trader_amount = trader_amount.to_sat(),
        "Proposing collaborative revert"
    );

    db::collaborative_reverts::insert(
        &mut conn,
        position::models::CollaborativeRevert {
            channel_id,
            trader_pubkey: peer_id,
            price: settlement_price.to_f32().expect("to fit into f32"),
            coordinator_address: coordinator_address.clone(),
            coordinator_amount_sats: coordinator_amount,
            trader_amount_sats: trader_amount,
            timestamp: OffsetDateTime::now_utc(),
            txid: funding_txo.txid,
            vout: funding_txo.vout,
        },
    )
    .context("Could not insert new collaborative revert")?;

    // Send collaborative revert proposal to the counterpary.
    sender
        .send(OrderbookMessage::TraderMessage {
            trader_id: peer_id,
            message: Message::CollaborativeRevert {
                channel_id,
                coordinator_address,
                coordinator_amount,
                trader_amount,
                execution_price: settlement_price,
                funding_txo,
            },
            notification: Some(NotificationKind::CollaborativeRevert),
        })
        .await
        .map_err(|error| anyhow!("Could send message to notify user {error:#}"))?;

    Ok(())
}

/// Propose collaboratively reverting the channel identified by `channel_id`, without LDK's
/// [`ChannelDetails`] for said channel.
///
/// A collaborative revert involves signing a new transaction spending from the funding output
/// directly. This can be used to circumvent bugs related to position and subchannel state.
#[allow(clippy::too_many_arguments)]
pub async fn propose_collaborative_revert_without_channel_details(
    node: Arc<Node<CoordinatorTenTenOneStorage, NodeStorage>>,
    pool: Pool<ConnectionManager<PgConnection>>,
    sender: mpsc::Sender<OrderbookMessage>,
    channel_id: [u8; 32],
    funding_txo: OutPoint,
    coordinator_amount: u64,
    fee_rate_sats_vb: u64,
    // The settlement price is purely informational for the counterparty.
    settlement_price: Decimal,
) -> Result<()> {
    let mut conn = pool.get().context("Could not acquire DB lock")?;

    let channel_id_hex = channel_id.to_hex();

    let subchannels = node
        .list_dlc_channels()
        .context("Could not get list of subchannels")?;

    let subchannel = subchannels
        .iter()
        .find(|c| c.channel_id == channel_id)
        .context("Missing subchannel")?;

    let peer_id = subchannel.counter_party;

    let trader_amount = subchannel.fund_value_satoshis - coordinator_amount;

    let fee = weight_to_fee(COLLABORATIVE_REVERT_TX_WEIGHT, fee_rate_sats_vb)
        .expect("To be able to calculate constant fee rate");

    let coordinator_address = node.get_unused_address();
    let coordinator_amount = Amount::from_sat(coordinator_amount - fee / 2);
    let trader_amount = Amount::from_sat(trader_amount - fee / 2);

    tracing::info!(
        channel_id = channel_id_hex,
        coordinator_address = %coordinator_address,
        coordinator_amount = coordinator_amount.to_sat(),
        trader_amount = trader_amount.to_sat(),
        "Proposing collaborative revert"
    );

    db::collaborative_reverts::insert(
        &mut conn,
        position::models::CollaborativeRevert {
            channel_id,
            trader_pubkey: peer_id,
            price: settlement_price.to_f32().expect("to fit into f32"),
            coordinator_address: coordinator_address.clone(),
            coordinator_amount_sats: coordinator_amount,
            trader_amount_sats: trader_amount,
            timestamp: OffsetDateTime::now_utc(),
            txid: funding_txo.txid,
            vout: funding_txo.vout,
        },
    )
    .context("Could not insert new collaborative revert")?;

    // Send collaborative revert proposal to the counterpary.
    sender
        .send(OrderbookMessage::TraderMessage {
            trader_id: peer_id,
            message: Message::CollaborativeRevert {
                channel_id,
                coordinator_address,
                coordinator_amount,
                trader_amount,
                execution_price: settlement_price,
                funding_txo,
            },
            notification: Some(NotificationKind::CollaborativeRevert),
        })
        .await
        .map_err(|error| anyhow!("Could send message to notify user {error:#}"))?;

    Ok(())
}

/// Complete the collaborative revert protocol by:
///
/// 1. Verifying the contents of the transaction sent by the counterparty.
/// 2. Signing the transaction.
/// 3. Broadcasting the signed transaction.
pub fn confirm_collaborative_revert(
    node: Arc<Node<CoordinatorTenTenOneStorage, NodeStorage>>,
    conn: &mut PooledConnection<ConnectionManager<PgConnection>>,
    channel_id: [u8; 32],
    mut revert_transaction: Transaction,
    counterparty_signature: Signature,
) -> Result<Transaction> {
    let channel_id_hex = channel_id.to_hex();

    tracing::info!(
        channel_id = channel_id_hex,
        txid = revert_transaction.txid().to_string(),
        "Confirming collaborative revert"
    );

    // TODO: Check if provided amounts are as expected.
    if !revert_transaction.output.iter().any(|output| {
        match node.wallet().is_mine(&output.script_pubkey) {
            Ok(is_mine) => is_mine,
            Err(e) => {
                tracing::error!(
                    "Failed to confirm if proposed collaborative revert \
                     transaction pays to the coordinator: {e:#}"
                );
                false
            }
        }
    }) {
        bail!("Proposed collaborative revert transaction doesn't pay the coordinator");
    }

    let subchannels = node
        .list_dlc_channels()
        .context("Failed to list subchannels")?;
    let subchannel = subchannels
        .iter()
        .find(|c| c.channel_id == channel_id)
        .with_context(|| format!("Could not find subchannel {channel_id_hex}"))?;

    let own_sig = {
        let channel_keys_id = subchannel
            .channel_keys_id
            .or(node
                .channel_manager
                .get_channel_details(&subchannel.channel_id)
                .map(|details| details.channel_keys_id))
            .with_context(|| {
                format!("Could not get channel keys ID for subchannel {channel_id_hex}")
            })?;

        let signer = node
            .keys_manager
            .derive_ln_dlc_channel_signer(subchannel.fund_value_satoshis, channel_keys_id);

        signer
            .get_holder_split_tx_signature(
                &Secp256k1::new(),
                &revert_transaction,
                &subchannel.original_funding_redeemscript,
                subchannel.fund_value_satoshis,
            )
            .context("Could not get own signature for collaborative revert transaction")?
    };

    let position = Position::get_position_by_trader(conn, subchannel.counter_party, vec![])?
        .with_context(|| format!("Could not load position for subchannel {channel_id_hex}"))?;

    dlc::util::finalize_multi_sig_input_transaction(
        &mut revert_transaction,
        vec![
            (subchannel.own_fund_pk, own_sig),
            (subchannel.counter_fund_pk, counterparty_signature),
        ],
        &subchannel.original_funding_redeemscript,
        0,
    );

    tracing::info!(
        txid = revert_transaction.txid().to_string(),
        "Broadcasting collaborative revert transaction"
    );
    node.wallet()
        .broadcast_transaction(&revert_transaction)
        .context("Could not broadcast transaction")?;

    // TODO: We should probably not modify the state until the transaction has been confirmed.

    Position::set_position_to_closed(conn, position.id)
        .context("Could not set position to closed")?;

    let mut subchannel = subchannel.clone();

    subchannel.state = SubChannelState::OnChainClosed;
    node.sub_channel_manager
        .get_dlc_manager()
        .get_store()
        .upsert_sub_channel(&subchannel)?;

    db::collaborative_reverts::delete(conn, channel_id)?;

    Ok(revert_transaction)
}

/// Estimate how many sats where reserved to pay for potential transaction fees when creating the
/// subchannel.
///
/// This fee was meant to be split evenly between both parties.
fn estimate_subchannel_reserved_tx_fees(
    fund_txo_sat: u64,
    inbound_capacity: u64,
    outbound_capacity: u64,
    reserve: u64,
    trader_margin: u64,
    coordinator_margin: u64,
) -> Result<u64> {
    let dlc_tx_fee = fund_txo_sat
        .checked_sub(inbound_capacity)
        .context("could not subtract inbound capacity")?
        .checked_sub(outbound_capacity)
        .context("could not subtract outbound capacity")?
        .checked_sub(reserve * 2)
        .context("could not subtract the reserve")?
        .checked_sub(trader_margin)
        .context("could not subtract trader margin")?
        .checked_sub(coordinator_margin)
        .context("could not subtract coordinator margin")?;

    Ok(dlc_tx_fee)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    pub fn estimate_subchannel_reserved_tx_fees_test() {
        let total_fee =
            estimate_subchannel_reserved_tx_fees(200_000, 65_450, 85_673, 1_000, 18_690, 18_690)
                .unwrap();
        assert_eq!(total_fee, 9_497);
    }

    #[test]
    pub fn estimate_subchannel_reserved_tx_fees_cannot_overflow() {
        assert!(estimate_subchannel_reserved_tx_fees(
            200_000, 84_140, 104_363, 1_000, 18_690, 18_690,
        )
        .is_err());
    }
}
