use crate::msg::TransportMsg;
use async_std::channel::{bounded, Receiver, Sender};
use bluesea_identity::{ConnId, NodeAddr, NodeId};
use std::sync::Arc;
use thiserror::Error;
use utils::error_handle::ErrorUtils;

pub struct TransportConnectingOutgoing {
    pub conn_id: ConnId,
}

pub enum TransportEvent {
    IncomingRequest(NodeId, ConnId, Box<dyn ConnectionAcceptor>),
    OutgoingRequest(NodeId, ConnId, Box<dyn ConnectionAcceptor>),
    Incoming(Arc<dyn ConnectionSender>, Box<dyn ConnectionReceiver + Send>),
    Outgoing(Arc<dyn ConnectionSender>, Box<dyn ConnectionReceiver + Send>),
    OutgoingError { node_id: NodeId, conn_id: ConnId, err: OutgoingConnectionError },
}

#[async_trait::async_trait]
pub trait Transport {
    fn connector(&self) -> Arc<dyn TransportConnector>;
    async fn recv(&mut self) -> Result<TransportEvent, ()>;
}

pub trait RpcAnswer<Res> {
    fn ok(&self, res: Res);
    fn error(&self, code: u32, message: &str);
}

#[async_trait::async_trait]
pub trait TransportRpc<Req, Res> {
    async fn recv(&mut self) -> Result<(u8, Req, Box<dyn RpcAnswer<Res>>), ()>;
}

pub trait TransportConnector: Send + Sync {
    fn connect_to(&self, node_id: NodeId, dest: NodeAddr) -> Result<TransportConnectingOutgoing, OutgoingConnectionError>;
}

#[derive(PartialEq, Debug, Clone)]
pub struct ConnectionStats {
    pub rtt_ms: u16,
    pub sending_kbps: u32,
    pub send_est_kbps: u32,
    pub loss_percent: u32,
    pub over_use: bool,
}

#[derive(PartialEq, Debug)]
pub enum ConnectionEvent {
    Msg(TransportMsg),
    Stats(ConnectionStats),
}

#[derive(PartialEq, Error, Clone, Debug)]
pub enum ConnectionRejectReason {
    #[error("Connection Limited")]
    ConnectionLimited,
    #[error("Validate Error")]
    ValidateError,
    #[error("Custom {0}")]
    Custom(String),
}

pub trait ConnectionAcceptor: Send + Sync {
    fn accept(&self);
    fn reject(&self, err: ConnectionRejectReason);
}

pub trait ConnectionSender: Send + Sync {
    fn remote_node_id(&self) -> NodeId;
    fn conn_id(&self) -> ConnId;
    fn remote_addr(&self) -> NodeAddr;
    fn send(&self, msg: TransportMsg);
    fn close(&self);
}

#[async_trait::async_trait]
pub trait ConnectionReceiver {
    fn remote_node_id(&self) -> NodeId;
    fn conn_id(&self) -> ConnId;
    fn remote_addr(&self) -> NodeAddr;
    async fn poll(&mut self) -> Result<ConnectionEvent, ()>;
}

#[derive(PartialEq, Error, Clone, Debug)]
pub enum OutgoingConnectionError {
    #[error("Too many connection")]
    TooManyConnection,
    #[error("Authentication Error")]
    AuthenticationError,
    #[error("Unsupported Protocol")]
    UnsupportedProtocol,
    #[error("Destination Not Found")]
    DestinationNotFound,
    #[error("Behavior Rejected")]
    BehaviorRejected(ConnectionRejectReason),
}

pub struct AsyncConnectionAcceptor {
    sender: Sender<Result<(), ConnectionRejectReason>>,
}

impl AsyncConnectionAcceptor {
    pub fn new() -> (Box<Self>, Receiver<Result<(), ConnectionRejectReason>>) {
        let (sender, receiver) = bounded(1);
        (Box::new(Self { sender }), receiver)
    }
}

impl ConnectionAcceptor for AsyncConnectionAcceptor {
    fn accept(&self) {
        self.sender.send_blocking(Ok(())).print_error("Should send accept");
    }

    fn reject(&self, err: ConnectionRejectReason) {
        self.sender.send_blocking(Err(err)).print_error("Should send reject");
    }
}
