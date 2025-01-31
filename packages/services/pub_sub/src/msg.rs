use crate::relay::ChannelIdentify;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq)]
pub enum PubsubServiceBehaviourEvent {}

#[derive(Debug, PartialEq, Eq)]
pub enum PubsubServiceHandlerEvent {}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum PubsubRemoteEvent {
    Sub(ChannelIdentify),
    Unsub(ChannelIdentify),
    SubAck(ChannelIdentify, bool),   //did it added, incase of false, it means it already subscribed
    UnsubAck(ChannelIdentify, bool), //did it removed, incase of false, it means it already unsubscribed
}

#[derive(Debug, PartialEq, Eq)]
pub enum PubsubSdkEvent {}
