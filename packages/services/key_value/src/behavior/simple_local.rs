use crate::{
    msg::{KeyValueSdkEventError, SimpleLocalEvent, SimpleRemoteEvent},
    KeyId, KeySource, KeyVersion, ReqId, ValueType,
};
use atm0s_sdn_identity::NodeId;
use atm0s_sdn_router::RouteRule;
use small_map::SmallMap;
/// This simple local storage is used for storing and act with remote storage
/// Main idea is we using sdk to act with local storage, and local storage will sync that data to remote
/// Local storage allow us to set/get/del/subscribe/unsubscribe
///
/// With Set, we will send Set event to remote storage, and wait for ack. If acked, we will set acked flag to true
/// With Del, we will send Del event to remote storage, and wait for ack. If acked, we will set acked flag to true
///
/// If we not received ack in time, we will resend event to remote storage in tick
///
/// With acked data we also sync data to remote storage in tick each sync_each_ms
/// Same with subscribe/unsubscribe
use std::{
    collections::{HashMap, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

struct KeySlotData {
    value: Option<Vec<u8>>,
    ex: Option<u64>,
    version: KeyVersion,
    last_sync: u64,
    acked: bool,
}

struct KeySlotSubscribe {
    ex: Option<u64>,
    last_sync: u64,
    sub: bool,
    acked: bool,
    handlers: SmallMap<16, (u64, u8), ()>,
    value: Option<(Vec<u8>, KeyVersion, KeySource)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SimpleKeyValueGetError {
    NetworkError,
    Timeout,
    InternalError,
}

struct KeySlotGetCallback {
    key: KeyId,
    timeout_after_ts: u64,
    uuid: u64,
    service_id: u8,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LocalStorageAction {
    SendNet(SimpleRemoteEvent, RouteRule),
    LocalOnChanged(u8, u64, KeyId, Option<ValueType>, KeyVersion, KeySource),
    LocalOnGet(u8, u64, KeyId, Result<Option<(ValueType, KeyVersion, KeySource)>, KeyValueSdkEventError>),
}

pub struct SimpleLocalStorage {
    req_id_seed: AtomicU64,
    version_seed: u16,
    sync_each_ms: u64,
    data: HashMap<KeyId, KeySlotData>,
    subscribe: HashMap<KeyId, KeySlotSubscribe>,
    output_events: VecDeque<LocalStorageAction>,
    get_queue: HashMap<ReqId, KeySlotGetCallback>,
}

impl SimpleLocalStorage {
    /// create new local storage with provided timer and sync_each_ms. Sync_each_ms is used for sync data to remote storage incase of acked
    pub fn new(sync_each_ms: u64) -> Self {
        Self {
            req_id_seed: AtomicU64::new(0),
            version_seed: 0,
            sync_each_ms,
            data: HashMap::new(),
            subscribe: HashMap::new(),
            output_events: VecDeque::new(),
            get_queue: HashMap::new(),
        }
    }

    fn gen_req_id(&self) -> u64 {
        return self.req_id_seed.fetch_add(1, Ordering::SeqCst);
    }

    fn gen_version(&mut self, now_ms: u64) -> u64 {
        let res = (now_ms << 16 | self.version_seed as u64) as u64;
        self.version_seed = self.version_seed.wrapping_add(1);
        return res;
    }

    /// Resend key releated event if not acked
    pub fn tick(&mut self, now: u64) {
        for (key, slot) in self.data.iter() {
            // we resend event each tick if not acked. If has data => Set, no data => Del
            if !slot.acked {
                let req_id = self.gen_req_id();
                if let Some(value) = &slot.value {
                    log::debug!("[SimpleLocal] resend set key {} with version {}", key, slot.version);
                    self.output_events.push_back(LocalStorageAction::SendNet(
                        SimpleRemoteEvent::Set(req_id, *key, value.clone(), slot.version, slot.ex.clone()),
                        RouteRule::ToKey(*key as u32),
                    ));
                } else {
                    log::debug!("[SimpleLocal] resend del key {} with version {}", key, slot.version);
                    self.output_events
                        .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Del(req_id, *key, slot.version), RouteRule::ToKey(*key as u32)));
                }
            }
        }

        for (key, slot) in self.subscribe.iter() {
            // we resend event each tick if not acked, corresponse with sub/unsub
            if !slot.acked {
                let req_id = self.gen_req_id();
                if slot.sub {
                    log::debug!("[SimpleLocal] resend sub key {}", key);
                    self.output_events
                        .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(req_id, *key, slot.ex.clone()), RouteRule::ToKey(*key as u32)));
                } else {
                    log::debug!("[SimpleLocal] resend unsub key {}", key);
                    self.output_events
                        .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Unsub(req_id, *key), RouteRule::ToKey(*key as u32)));
                }
            }
        }

        // we sync data each sync_each_ms with each data which acked
        let mut removed_keys = Vec::new();
        for (key, slot) in self.data.iter() {
            if slot.acked && now - slot.last_sync >= self.sync_each_ms {
                let req_id = self.gen_req_id();
                if let Some(value) = &slot.value {
                    log::debug!("[SimpleLocal] sync set key {} with version {}", key, slot.version);
                    self.output_events.push_back(LocalStorageAction::SendNet(
                        SimpleRemoteEvent::Set(req_id, *key, value.clone(), slot.version, slot.ex.clone()),
                        RouteRule::ToKey(*key as u32),
                    ));
                } else {
                    log::debug!("[SimpleLocal] del key {} with version {} after acked", key, slot.version);
                    // Just removed if acked and no data
                    removed_keys.push(*key);
                }
            }
        }

        // we set last_sync here for avoid borrowed mutable Self twice
        for (_key, slot) in self.data.iter_mut() {
            if slot.acked && now - slot.last_sync >= self.sync_each_ms {
                slot.last_sync = now;
            }
        }

        let mut unsub_keys = Vec::new();
        // we sync subscribe each sync_each_ms with each subscribe which acked
        for (key, slot) in self.subscribe.iter() {
            if slot.acked && now - slot.last_sync >= self.sync_each_ms {
                let req_id = self.gen_req_id();
                if slot.sub {
                    log::debug!("[SimpleLocal] sync sub key {}", key);
                    self.output_events
                        .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(req_id, *key, slot.ex.clone()), RouteRule::ToKey(*key as u32)));
                } else {
                    log::debug!("[SimpleLocal] remove sub key {} after acked", key);
                    // Just remove if acked and unsub
                    unsub_keys.push(*key);
                }
            }
        }

        // we set last_sync here for avoid borrowed mutable Self twice
        for (_key, slot) in self.subscribe.iter_mut() {
            if slot.acked && now - slot.last_sync >= self.sync_each_ms {
                slot.last_sync = now;
            }
        }

        // we get timeout getter
        let mut timeout_gets = Vec::new();
        for (req_id, slot) in self.get_queue.iter() {
            if now >= slot.timeout_after_ts {
                timeout_gets.push(*req_id);
            }
        }

        // we clear timeout getter
        for req_id in timeout_gets {
            if let Some(slot) = self.get_queue.remove(&req_id) {
                log::debug!("[SimpleLocal] get key {} timeout", req_id);
                self.output_events
                    .push_back(LocalStorageAction::LocalOnGet(slot.service_id, slot.uuid, slot.key, Err(KeyValueSdkEventError::Timeout)));
            }
        }

        for key in removed_keys {
            self.data.remove(&key);
        }

        for key in unsub_keys {
            self.subscribe.remove(&key);
        }
    }

    pub fn on_event(&mut self, from: NodeId, event: SimpleLocalEvent) {
        log::debug!("[SimpleLocal] on_event from {} {:?}", from, event);

        match event {
            SimpleLocalEvent::SetAck(_req_id, key, version, success) => {
                if success {
                    if let Some(slot) = self.data.get_mut(&key) {
                        // we acked if version match
                        if slot.version == version {
                            slot.acked = true;
                        }
                    }
                } else {
                    // TODO: we should avoid race condition here, when multiple node set with same key
                    // let new_version = self.gen_version();
                    // if let Some(slot) = self.data.get_mut(&key) {
                    //     // we regenete if version match, because of remote reject that version
                    //     if slot.version < version {
                    //         slot.version = new_version;
                    //     }
                    // }
                }
            }
            SimpleLocalEvent::GetAck(req_id, _key, value) => {
                if let Some(slot) = self.get_queue.remove(&req_id) {
                    self.output_events.push_back(LocalStorageAction::LocalOnGet(slot.service_id, slot.uuid, slot.key, Ok(value)));
                } else {
                }
            }
            SimpleLocalEvent::DelAck(_req_id, key, version) => {
                if let Some(slot) = self.data.get_mut(&key) {
                    if let Some(deleted_version) = version {
                        // we acked if deleted version older than current version
                        if slot.version >= deleted_version {
                            slot.acked = true;
                        }
                    } else {
                        // incase of NoneKeyVersion, we just acked
                        slot.acked = true;
                    }
                }
            }
            SimpleLocalEvent::SubAck(_req_id, key_id) => {
                if let Some(slot) = self.subscribe.get_mut(&key_id) {
                    if slot.sub {
                        slot.acked = true;
                    }
                }
            }
            SimpleLocalEvent::UnsubAck(_req_id, key_id, success) => {
                if success {
                    if let Some(slot) = self.subscribe.get_mut(&key_id) {
                        if slot.sub == false {
                            slot.acked = true;
                        }
                    }
                }
            }
            SimpleLocalEvent::OnKeySet(req_id, key, value, version, source) => {
                self.output_events
                    .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::OnKeySetAck(req_id), RouteRule::ToNode(from)));
                if let Some(slot) = self.subscribe.get_mut(&key) {
                    slot.value = Some((value.clone(), version, source));
                    if slot.sub {
                        for ((uuid, service_id), _) in slot.handlers.iter() {
                            self.output_events
                                .push_back(LocalStorageAction::LocalOnChanged(*service_id, *uuid, key, Some(value.clone()), version, source));
                        }
                    }
                }
            }
            SimpleLocalEvent::OnKeyDel(req_id, key, version, source) => {
                self.output_events
                    .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::OnKeyDelAck(req_id), RouteRule::ToNode(from)));
                if let Some(slot) = self.subscribe.get_mut(&key) {
                    slot.value = None;
                    if slot.sub {
                        for ((uuid, service_id), _) in slot.handlers.iter() {
                            self.output_events.push_back(LocalStorageAction::LocalOnChanged(*service_id, *uuid, key, None, version, source));
                        }
                    }
                }
            }
        }
    }

    pub fn pop_action(&mut self) -> Option<LocalStorageAction> {
        self.output_events.pop_front()
    }

    pub fn set(&mut self, now_ms: u64, key: KeyId, value: ValueType, ex: Option<u64>) {
        let req_id = self.gen_req_id();
        let version = self.gen_version(now_ms);
        log::debug!("[SimpleLocal] set key {} with version {}", key, version);
        self.data.insert(
            key,
            KeySlotData {
                value: Some(value.clone()),
                ex,
                version,
                last_sync: 0,
                acked: false,
            },
        );

        self.output_events
            .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Set(req_id, key, value, version, ex), RouteRule::ToKey(key as u32)));
    }

    pub fn get(&mut self, now_ms: u64, key: KeyId, uuid: u64, service_id: u8, timeout_ms: u64) {
        let req_id = self.gen_req_id();
        log::debug!("[SimpleLocal] get key {} with req_id {}", key, req_id);
        self.get_queue.insert(
            req_id,
            KeySlotGetCallback {
                key,
                timeout_after_ts: now_ms + timeout_ms,
                uuid,
                service_id,
            },
        );
        self.output_events
            .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Get(req_id, key), RouteRule::ToKey(key as u32)));
    }

    pub fn del(&mut self, key: KeyId) {
        let req_id = self.gen_req_id();
        log::debug!("[SimpleLocal] del key {} with req_id {}", key, req_id);
        if let Some(slot) = self.data.get_mut(&key) {
            slot.value = None;
            slot.last_sync = 0;
            slot.acked = false;

            self.output_events
                .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Del(req_id, key, slot.version), RouteRule::ToKey(key as u32)));
        }
    }

    pub fn subscribe(&mut self, key: KeyId, ex: Option<u64>, uuid: u64, service_id: u8) {
        if let Some(slot) = self.subscribe.get_mut(&key) {
            log::debug!("[SimpleLocal] subscribe key {} but already subscribed => only add to handers list", key);
            slot.handlers.insert((uuid, service_id), ());
            if let Some((value, version, source)) = &slot.value {
                self.output_events
                    .push_back(LocalStorageAction::LocalOnChanged(service_id, uuid, key, Some(value.clone()), *version, *source));
            }
            return;
        }

        let req_id = self.gen_req_id();
        log::debug!("[SimpleLocal] subscribe key {} with req_id {}", key, req_id);
        let mut handlers = SmallMap::new();
        handlers.insert((uuid, service_id), ());
        self.subscribe.insert(
            key,
            KeySlotSubscribe {
                ex,
                last_sync: 0,
                sub: true,
                acked: false,
                handlers,
                value: None,
            },
        );
        self.output_events
            .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(req_id, key, ex), RouteRule::ToKey(key as u32)));
    }

    pub fn unsubscribe(&mut self, key: KeyId, uuid: u64, service_id: u8) {
        let req_id = self.gen_req_id();
        if let Some(slot) = self.subscribe.get_mut(&key) {
            slot.handlers.remove(&(uuid, service_id));
            if slot.handlers.is_empty() {
                slot.sub = false;
                slot.last_sync = 0;
                slot.acked = false;

                log::debug!("[SimpleLocal] unsubscribe key {} with req_id {}", key, req_id);

                self.output_events
                    .push_back(LocalStorageAction::SendNet(SimpleRemoteEvent::Unsub(req_id, key), RouteRule::ToKey(key as u32)));
            }
        } else {
            log::warn!("[SimpleLocal] unsubscribe key {} but not subscribed", key);
        }
    }
}

#[cfg(test)]
mod tests {
    use atm0s_sdn_router::RouteRule;

    use crate::{
        behavior::simple_local::LocalStorageAction,
        msg::{KeyValueSdkEventError, SimpleLocalEvent, SimpleRemoteEvent},
    };

    use super::SimpleLocalStorage;

    #[test]
    fn set_should_mark_after_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);

        assert_eq!(
            storage.pop_action(),
            Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Set(0, 1, vec![1], 0, None), RouteRule::ToKey(1)))
        );
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SetAck(0, 1, 0, true));

        //after received ack should not resend event
        storage.tick(100);
        assert_eq!(storage.pop_action(), None);
    }

    // #[test]
    // fn should_renegerate_set_event_if_ack_failed() {
    //     let timer = Arc::new(utils::MockTimer::default());
    //     let awake_notify = Arc::new(MockAwaker::default());
    //     let mut storage = LocalStorage::new(10000);

    //     storage.set(1, vec![1], None);
    //     assert_eq!(storage.pop_action(), Some(LocalStorageAction(RemoteEvent::Set(0, 1, vec![1], 0, None), RouteRule::ToKey(1))));
    //     assert_eq!(storage.pop_action(), None);

    //     storage.on_event(2, LocalEvent::SetAck(0, 1, 0, false));

    //     //after received ack with failed => should regenerate new version
    //     storage.tick();
    //     assert_eq!(storage.pop_action(), Some(LocalStorageAction(RemoteEvent::Set(1, 1, vec![1], 1, None), RouteRule::ToKey(1))));
    //     assert_eq!(storage.pop_action(), None);

    //     storage.on_event(2, LocalEvent::SetAck(1, 1, 1, true));
    //     storage.tick();
    //     assert_eq!(storage.pop_action(), None);
    // }

    #[test]
    fn set_should_generate_new_version() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());

        storage.set(1000, 1, vec![2], None);
        assert_eq!(
            storage.pop_action(),
            Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Set(1, 1, vec![2], 65536001, None), RouteRule::ToKey(1)))
        );
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SetAck(1, 1, 65536001, true));

        //after received ack should not resend event
        storage.tick(1000);
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn set_should_retry_after_tick_and_not_received_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());

        //because dont received ack, should resend event
        storage.tick(0);
        assert_eq!(
            storage.pop_action(),
            Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Set(1, 1, vec![1], 0, None), RouteRule::ToKey(1)))
        );
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn set_acked_should_resend_each_sync_each_ms() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());

        storage.on_event(2, SimpleLocalEvent::SetAck(0, 1, 0, true));

        //after received ack should not resend event
        storage.tick(0);
        assert_eq!(storage.pop_action(), None);

        //should resend if timer greater than sync_each_ms
        storage.tick(10001);
        assert_eq!(
            storage.pop_action(),
            Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Set(1, 1, vec![1], 0, None), RouteRule::ToKey(1)))
        );
    }

    #[test]
    fn del_should_mark_after_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());
        storage.on_event(2, SimpleLocalEvent::SetAck(0, 1, 0, true));

        storage.del(1);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Del(1, 1, 0), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        //after received ack should not resend event
        storage.on_event(2, SimpleLocalEvent::DelAck(0, 1, Some(0)));
        storage.tick(0);
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn del_should_mark_after_ack_older() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());
        storage.on_event(2, SimpleLocalEvent::SetAck(0, 1, 0, true));

        storage.set(1000, 1, vec![2], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());
        storage.on_event(2, SimpleLocalEvent::SetAck(0, 1, 0, true));

        storage.del(1);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Del(2, 1, 65536001), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        //after received ack should not resend event
        storage.on_event(2, SimpleLocalEvent::DelAck(2, 1, Some(65536001)));
        storage.tick(1000);
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn del_should_retry_after_tick_and_not_received_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.set(0, 1, vec![1], None);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());
        storage.on_event(2, SimpleLocalEvent::SetAck(0, 1, 0, true));

        storage.del(1);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Del(1, 1, 0), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.tick(0);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Del(2, 1, 0), RouteRule::ToKey(1))));
    }

    #[test]
    fn sub_should_mark_after_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.subscribe(1, None, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(0, 1, None), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        storage.tick(0);
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn sub_event_test() {
        let mut storage = SimpleLocalStorage::new(10000);
        storage.subscribe(1, None, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(0, 1, None), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        storage.tick(0);
        assert_eq!(storage.pop_action(), None);

        // fake incoming event
        storage.on_event(2, SimpleLocalEvent::OnKeySet(0, 1, vec![1], 0, 1000));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::OnKeySetAck(0), RouteRule::ToNode(2))));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnChanged(10, 11111, 1, Some(vec![1]), 0, 1000)));

        storage.on_event(2, SimpleLocalEvent::OnKeyDel(1, 1, 0, 1000));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::OnKeyDelAck(1), RouteRule::ToNode(2))));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnChanged(10, 11111, 1, None, 0, 1000)));

        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn sub_multi_times_test() {
        let mut storage = SimpleLocalStorage::new(10000);
        storage.subscribe(1, None, 11111, 10);
        storage.subscribe(1, None, 22222, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(0, 1, None), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        // fake incoming event
        storage.on_event(2, SimpleLocalEvent::OnKeySet(0, 1, vec![1], 0, 1000));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::OnKeySetAck(0), RouteRule::ToNode(2))));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnChanged(10, 11111, 1, Some(vec![1]), 0, 1000)));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnChanged(10, 22222, 1, Some(vec![1]), 0, 1000)));
    }

    #[test]
    fn sub_multi_times_after_test() {
        let mut storage = SimpleLocalStorage::new(10000);
        storage.subscribe(1, None, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(0, 1, None), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        // fake incoming event
        storage.on_event(2, SimpleLocalEvent::OnKeySet(0, 1, vec![1], 0, 1000));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::OnKeySetAck(0), RouteRule::ToNode(2))));
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnChanged(10, 11111, 1, Some(vec![1]), 0, 1000)));
        assert_eq!(storage.pop_action(), None);

        storage.subscribe(1, None, 22222, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnChanged(10, 22222, 1, Some(vec![1]), 0, 1000)));
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn sub_should_retry_after_tick_and_not_received_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.subscribe(1, None, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(0, 1, None), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.tick(0);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(1, 1, None), RouteRule::ToKey(1))));
    }

    #[test]
    fn sub_acked_should_resend_each_sync_each_ms() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.subscribe(1, None, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(0, 1, None), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        storage.tick(0);
        assert_eq!(storage.pop_action(), None);

        //should resend if timer greater than sync_each_ms
        storage.tick(10001);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Sub(1, 1, None), RouteRule::ToKey(1))));
    }

    #[test]
    fn unsub_should_mark_after_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.subscribe(1, None, 11111, 10);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        //sending unsub
        storage.unsubscribe(1, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Unsub(1, 1), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        //after received ack should not resend event
        storage.on_event(2, SimpleLocalEvent::UnsubAck(1, 1, true));
        storage.tick(0);
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn unsub_should_retry_after_tick_if_not_received_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.subscribe(1, None, 11111, 10);
        assert!(storage.pop_action().is_some());
        assert!(storage.pop_action().is_none());

        storage.on_event(2, SimpleLocalEvent::SubAck(0, 1));

        //sending unsub
        storage.unsubscribe(1, 11111, 10);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Unsub(1, 1), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        //if not received ack should resend event each tick
        storage.tick(0);
        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Unsub(2, 1), RouteRule::ToKey(1))));
    }

    #[test]
    fn get_should_callback_correct_value() {
        let mut storage = SimpleLocalStorage::new(10000);
        storage.get(0, 1, 11111, 10, 1000);

        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Get(0, 1), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        //fake received result
        storage.on_event(2, SimpleLocalEvent::GetAck(0, 1, Some((vec![1], 0, 1000))));

        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnGet(10, 11111, 1, Ok(Some((vec![1], 0, 1000))))));
        assert_eq!(storage.pop_action(), None);
    }

    #[test]
    fn get_should_timeout_after_no_ack() {
        let mut storage = SimpleLocalStorage::new(10000);

        storage.get(0, 1, 11111, 10, 1000);

        assert_eq!(storage.pop_action(), Some(LocalStorageAction::SendNet(SimpleRemoteEvent::Get(0, 1), RouteRule::ToKey(1))));
        assert_eq!(storage.pop_action(), None);

        //after timeout should callback error
        storage.tick(1001);

        assert_eq!(storage.pop_action(), Some(LocalStorageAction::LocalOnGet(10, 11111, 1, Err(KeyValueSdkEventError::Timeout))));
        assert_eq!(storage.pop_action(), None);
    }
}
