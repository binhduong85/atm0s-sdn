use async_std::channel::{Receiver, Sender};
use atm0s_sdn_identity::{ConnId, NodeAddr, NodeId};
use atm0s_sdn_network::msg::TransportMsg;
use atm0s_sdn_network::transport::{ConnectionEvent, ConnectionReceiver, ConnectionSender, ConnectionStats};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

pub type VnetConnection = (Arc<VnetConnectionSender>, Box<VnetConnectionReceiver>);

pub struct VnetConnectionReceiver {
    pub(crate) remote_node_id: NodeId,
    pub(crate) conn_id: ConnId,
    pub(crate) remote_addr: NodeAddr,
    pub(crate) recv: Receiver<Option<TransportMsg>>,
    pub(crate) connections: Arc<RwLock<HashMap<ConnId, (NodeId, NodeId)>>>,
    pub(crate) first_stats: Option<ConnectionStats>,
}

#[async_trait::async_trait]
impl ConnectionReceiver for VnetConnectionReceiver {
    fn remote_node_id(&self) -> atm0s_sdn_identity::NodeId {
        self.remote_node_id
    }

    fn conn_id(&self) -> ConnId {
        self.conn_id
    }

    fn remote_addr(&self) -> atm0s_sdn_identity::NodeAddr {
        self.remote_addr.clone()
    }

    async fn poll(&mut self) -> Result<ConnectionEvent, ()> {
        if let Some(stats) = self.first_stats.take() {
            return Ok(ConnectionEvent::Stats(stats));
        }

        if let Some(msg) = self.recv.recv().await.map_err(|_e| ())? {
            Ok(ConnectionEvent::Msg(msg))
        } else {
            //disconnected
            self.connections.write().remove(&self.conn_id);
            Err(())
        }
    }
}

pub struct VnetConnectionSender {
    pub(crate) remote_node_id: NodeId,
    pub(crate) conn_id: ConnId,
    pub(crate) remote_addr: NodeAddr,
    pub(crate) sender: Sender<Option<TransportMsg>>,
    pub(crate) remote_sender: Sender<Option<TransportMsg>>,
}

#[async_trait::async_trait]
impl ConnectionSender for VnetConnectionSender {
    fn remote_node_id(&self) -> atm0s_sdn_identity::NodeId {
        self.remote_node_id
    }

    fn conn_id(&self) -> ConnId {
        self.conn_id
    }

    fn remote_addr(&self) -> atm0s_sdn_identity::NodeAddr {
        self.remote_addr.clone()
    }

    fn send(&self, msg: TransportMsg) {
        self.remote_sender.send_blocking(Some(msg)).unwrap();
    }

    fn close(&self) {
        self.sender.send_blocking(None).unwrap();
        self.remote_sender.send_blocking(None).unwrap();
    }
}
