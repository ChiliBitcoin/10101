use crate::event;
use crate::event::EventInternal;
use crate::ln_dlc::node::Node;
use lightning::events::Event;
use tokio::sync::watch::Receiver;

impl Node {
    pub async fn listen_for_lightning_events(&self, mut event_receiver: Receiver<Option<Event>>) {
        loop {
            match event_receiver.changed().await {
                Ok(()) => {
                    if let Some(event) = event_receiver.borrow().clone() {
                        match event {
                            Event::ChannelReady { channel_id, .. } => {
                                event::publish(&EventInternal::ChannelReady(channel_id))
                            }
                            Event::PaymentClaimed {
                                amount_msat,
                                payment_hash,
                                ..
                            } => event::publish(&EventInternal::PaymentClaimed(
                                amount_msat,
                                payment_hash,
                            )),
                            Event::PaymentSent { .. } => {
                                event::publish(&EventInternal::PaymentSent)
                            }
                            Event::PaymentFailed { .. } => {
                                event::publish(&EventInternal::PaymentFailed)
                            }
                            Event::SpendableOutputs { .. } => {
                                event::publish(&EventInternal::SpendableOutputs)
                            }
                            _ => tracing::trace!("Ignoring event on the mobile app"),
                        }
                    }
                }
                Err(_) => {
                    tracing::error!("Sender died, channel closed.");
                    break;
                }
            }
        }
    }
}
