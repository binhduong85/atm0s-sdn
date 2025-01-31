use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};

use async_std::{channel::Receiver, net::UdpSocket, stream::StreamExt};
use atm0s_sdn_identity::{ConnId, NodeAddr, NodeId};
use atm0s_sdn_network::{
    msg::TransportMsg,
    transport::{ConnectionEvent, ConnectionReceiver, ConnectionStats},
};
use atm0s_sdn_utils::{error_handle::ErrorUtils, Timer};
use futures_util::{select, FutureExt};
use parking_lot::Mutex;
use snow::TransportState;

use crate::msg::{build_control_msg, UdpTransportMsg};

pub struct UdpServerConnectionReceiver {
    closed: bool,
    rx: Receiver<([u8; 1500], usize)>,
    socket: Arc<UdpSocket>,
    socket_dest: SocketAddr,
    conn_id: ConnId,
    remote_node_id: NodeId,
    remote_node_addr: NodeAddr,
    timer: Arc<dyn Timer>,
    tick: async_std::stream::Interval,
    close_state: Arc<AtomicBool>,
    close_notify: Arc<async_notify::Notify>,
    last_pong_ts: u64,
    snow_state: Arc<Mutex<TransportState>>,
    snow_buf: [u8; 1500],
}

impl UdpServerConnectionReceiver {
    pub fn new(
        socket: Arc<UdpSocket>,
        socket_dest: SocketAddr,
        rx: Receiver<([u8; 1500], usize)>,
        conn_id: ConnId,
        remote_node_id: NodeId,
        remote_node_addr: NodeAddr,
        timer: Arc<dyn Timer>,
        close_state: Arc<AtomicBool>,
        close_notify: Arc<async_notify::Notify>,
        snow_state: Arc<Mutex<TransportState>>,
    ) -> Self {
        log::info!("[UdpServerConnectionReceiver {}/{}] new", remote_node_id, conn_id);

        Self {
            closed: false,
            socket,
            socket_dest,
            rx,
            conn_id,
            remote_node_id,
            remote_node_addr,
            last_pong_ts: timer.now_ms(),
            timer,
            tick: async_std::stream::interval(std::time::Duration::from_secs(1)),
            close_state,
            close_notify,
            snow_state,
            snow_buf: [0u8; 1500],
        }
    }
}

#[async_trait::async_trait]
impl ConnectionReceiver for UdpServerConnectionReceiver {
    fn remote_node_id(&self) -> NodeId {
        self.remote_node_id
    }
    fn conn_id(&self) -> ConnId {
        self.conn_id
    }
    fn remote_addr(&self) -> NodeAddr {
        self.remote_node_addr.clone()
    }
    async fn poll(&mut self) -> Result<ConnectionEvent, ()> {
        if self.closed {
            return Err(());
        }

        loop {
            select! {
                _ = self.close_notify.notified().fuse() => {
                    log::info!("[UdpServerConnectionReceiver {}/{}] close notify received", self.remote_node_id, self.conn_id);
                    self.closed = true;
                    break Err(());
                }
                _ = self.tick.next().fuse() => {
                    if self.last_pong_ts + 10000 < self.timer.now_ms() {
                        self.closed = true;
                        self.close_state.store(true, std::sync::atomic::Ordering::SeqCst);
                        log::info!("[UdpServerConnectionReceiver {}/{}] timeout => close", self.remote_node_id, self.conn_id);
                        break Err(());
                    }

                    self.socket.send_to(&build_control_msg(&UdpTransportMsg::Ping(self.timer.now_ms())), self.socket_dest).await.print_error("Should send Ping");
                },
                e = self.rx.recv().fuse() => match e {
                    Ok((data, len)) => {
                        if len > 0 {
                            if data[0] == 255 {
                                match bincode::deserialize::<UdpTransportMsg>(&data[1..len]) {
                                    Ok(UdpTransportMsg::Ping(ts)) => {
                                        log::debug!("[UdpServerConnectionReceiver {}/{}] on ping received {}", self.remote_node_id, self.conn_id, ts);
                                        self.socket.send_to(&build_control_msg(&UdpTransportMsg::Pong(ts)), self.socket_dest).await.print_error("Should send Pong");
                                    }
                                    Ok(UdpTransportMsg::Pong(ts)) => {
                                        self.last_pong_ts = self.timer.now_ms();
                                        log::debug!("[UdpServerConnectionReceiver {}/{}] on pong received {} ms", self.remote_node_id, self.conn_id, self.last_pong_ts - ts);
                                        //TODO est speed and over_use state
                                        break Ok(ConnectionEvent::Stats(ConnectionStats {
                                            rtt_ms: (self.last_pong_ts - ts) as u16,
                                            sending_kbps: 0,
                                            send_est_kbps: 0,
                                            loss_percent: 0,
                                            over_use: false,
                                        }));
                                    }
                                    Ok(UdpTransportMsg::Close) => {
                                        self.closed = true;
                                        self.close_state.store(true, std::sync::atomic::Ordering::SeqCst);
                                        log::info!("[UdpServerConnectionReceiver {}/{}] remove close received", self.remote_node_id, self.conn_id);
                                        break Err(());
                                    }
                                    _ => {}
                                }
                            } else {
                                if TransportMsg::is_secure_header(data[0]) {
                                    let mut snow_state = self.snow_state.lock();
                                    if let Ok(len) = snow_state.read_message(&data[1..len], &mut self.snow_buf) {
                                        //TODO reduce to_vec memory copy
                                        match TransportMsg::from_vec(self.snow_buf[0..len].to_vec()) {
                                            Ok(msg) => break Ok(ConnectionEvent::Msg(msg)),
                                            Err(e) => {
                                                log::error!("[UdpServerConnectionReceiver {}/{}] wrong msg format {:?}", self.remote_node_id, self.conn_id, e);
                                            }
                                        }
                                    }
                                } else {
                                    //TODO reduce to_vec memory copy
                                    match TransportMsg::from_vec(data[0..len].to_vec()) {
                                        Ok(msg) => break Ok(ConnectionEvent::Msg(msg)),
                                        Err(e) => {
                                            log::error!("[UdpServerConnectionReceiver {}/{}] wrong msg format {:?}", self.remote_node_id, self.conn_id, e);
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Err(err) => {
                        log::warn!("[UdpServerConnectionReceiver {}/{}] internal channel error {:?}", self.remote_node_id, self.conn_id, err);
                        self.closed = true;
                        return Err(());
                    }
                }
            }
        }
    }
}

impl Drop for UdpServerConnectionReceiver {
    fn drop(&mut self) {
        log::info!("[UdpServerConnectionReceiver {}/{}] drop", self.remote_node_id, self.conn_id);
    }
}

pub struct UdpClientConnectionReceiver {
    closed: bool,
    socket: Arc<UdpSocket>,
    conn_id: ConnId,
    remote_node_id: NodeId,
    remote_node_addr: NodeAddr,
    timer: Arc<dyn Timer>,
    tick: async_std::stream::Interval,
    close_state: Arc<AtomicBool>,
    close_notify: Arc<async_notify::Notify>,
    last_pong_ts: u64,
    snow_state: Arc<Mutex<TransportState>>,
    snow_buf: [u8; 1500],
}

impl UdpClientConnectionReceiver {
    pub fn new(
        socket: Arc<UdpSocket>,
        conn_id: ConnId,
        remote_node_id: NodeId,
        remote_node_addr: NodeAddr,
        timer: Arc<dyn Timer>,
        close_state: Arc<AtomicBool>,
        close_notify: Arc<async_notify::Notify>,
        snow_state: Arc<Mutex<TransportState>>,
    ) -> Self {
        log::info!("[UdpClientConnectionReceiver {}] new", remote_node_id);

        Self {
            closed: false,
            socket,
            conn_id,
            remote_node_id,
            remote_node_addr,
            last_pong_ts: timer.now_ms(),
            timer,
            tick: async_std::stream::interval(std::time::Duration::from_secs(1)),
            close_state,
            close_notify,
            snow_state,
            snow_buf: [0u8; 1500],
        }
    }
}

#[async_trait::async_trait]
impl ConnectionReceiver for UdpClientConnectionReceiver {
    fn remote_node_id(&self) -> NodeId {
        self.remote_node_id
    }
    fn conn_id(&self) -> ConnId {
        self.conn_id
    }
    fn remote_addr(&self) -> NodeAddr {
        self.remote_node_addr.clone()
    }
    async fn poll(&mut self) -> Result<ConnectionEvent, ()> {
        if self.closed {
            return Err(());
        }

        let mut data = [0; 1500];
        loop {
            select! {
                _ = self.close_notify.notified().fuse() => {
                    self.closed = true;
                    log::info!("[UdpClientConnectionReceiver {}] close notify received", self.remote_node_id);
                    break Err(());
                }
                _ = self.tick.next().fuse() => {
                    if self.last_pong_ts + 10000 < self.timer.now_ms() {
                        self.closed = true;
                        self.close_state.store(true, std::sync::atomic::Ordering::SeqCst);
                        log::info!("[UdpClientConnectionReceiver {}] timeout => close", self.remote_node_id);
                        break Err(());
                    }
                    self.socket.send(&build_control_msg(&UdpTransportMsg::Ping(self.timer.now_ms()))).await.print_error("Should send Ping");
                },
                e = self.socket.recv(&mut data).fuse() => match e {
                    Ok(len) => {
                        if len > 0 {
                            if data[0] == 255 {
                                match bincode::deserialize::<UdpTransportMsg>(&data[1..len]) {
                                    Ok(UdpTransportMsg::ConnectResponse(_, _)) => {
                                        self.socket
                                            .send(&build_control_msg(&UdpTransportMsg::ConnectResponseAck(true)))
                                            .await
                                            .print_error("Should send ConnectResponseAck");
                                    }
                                    Ok(UdpTransportMsg::Ping(ts)) => {
                                        log::debug!("[UdpClientConnectionReceiver {}] on ping received {}", self.remote_node_id, ts);
                                        self.socket.send(&build_control_msg(&UdpTransportMsg::Pong(ts))).await.print_error("Should send Pong");
                                    }
                                    Ok(UdpTransportMsg::Pong(ts)) => {
                                        self.last_pong_ts = self.timer.now_ms();
                                        log::debug!("[UdpClientConnectionReceiver {}] on pong received {} ms", self.remote_node_id, self.last_pong_ts - ts);
                                        //TODO est speed and over_use state
                                        break Ok(ConnectionEvent::Stats(ConnectionStats {
                                            rtt_ms: (self.timer.now_ms() - ts) as u16,
                                            sending_kbps: 0,
                                            send_est_kbps: 0,
                                            loss_percent: 0,
                                            over_use: false,
                                        }));
                                    }
                                    Ok(UdpTransportMsg::Close) => {
                                        self.closed = true;
                                        self.close_state.store(true, std::sync::atomic::Ordering::SeqCst);
                                        log::info!("[UdpClientConnectionReceiver {}] remove close received", self.remote_node_id);
                                        break Err(());
                                    }
                                    _ => {}
                                }
                            } else {
                                if TransportMsg::is_secure_header(data[0]) {
                                    let mut snow_state = self.snow_state.lock();
                                    if let Ok(len) = snow_state.read_message(&data[1..len], &mut self.snow_buf) {
                                        //TODO reduce to_vec memory copy
                                        match TransportMsg::from_vec(self.snow_buf[0..len].to_vec()) {
                                            Ok(msg) => break Ok(ConnectionEvent::Msg(msg)),
                                            Err(e) => {
                                                log::error!("[UdpClientConnectionReceiver {}] wrong msg format {:?}", self.remote_node_id, e);
                                            }
                                        }
                                    }
                                } else {
                                    //TODO reduce to_vec memory copy
                                    match TransportMsg::from_vec(data[0..len].to_vec()) {
                                        Ok(msg) => break Ok(ConnectionEvent::Msg(msg)),
                                        Err(e) => {
                                            log::error!("[UdpClientConnectionReceiver {}] wrong msg format {:?}", self.remote_node_id, e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        self.closed = true;
                        self.close_state.store(true, std::sync::atomic::Ordering::SeqCst);
                        log::warn!("[UdpClientConnectionReceiver {}] socket error {:?}", self.remote_node_id, err);
                        return Err(());
                    }
                }
            }
        }
    }
}

impl Drop for UdpClientConnectionReceiver {
    fn drop(&mut self) {
        log::info!("[UdpClientConnectionReceiver {}] drop", self.remote_node_id);
    }
}
