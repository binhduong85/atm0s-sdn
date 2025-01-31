use std::{
    collections::{HashMap, VecDeque},
    sync::{atomic::AtomicU64, Arc},
};

use async_std::channel::Sender;
use atm0s_sdn_utils::awaker::Awaker;
use parking_lot::{Mutex, RwLock};

use crate::{msg::KeyValueSdkEventError, ExternalControl, KeyId, KeySource, KeyValueSdkEvent, KeyVersion, SubKeyId, ValueType};

use super::{hashmap_local::HashmapKeyValueGetError, simple_local::SimpleKeyValueGetError};

mod pub_sub;

pub type SimpleKeyValueSubscriber = pub_sub::Subscriber<u64, (KeyId, Option<ValueType>, KeyVersion, KeySource)>;
pub type HashmapKeyValueSubscriber = pub_sub::Subscriber<u64, (KeyId, SubKeyId, Option<ValueType>, KeyVersion, KeySource)>;

#[derive(Clone)]
pub struct KeyValueSdk {
    req_id_gen: Arc<AtomicU64>,
    uuid_gen: Arc<AtomicU64>,
    awaker: Arc<RwLock<Option<Arc<dyn Awaker>>>>,
    simple_publisher: Arc<pub_sub::PublisherManager<u64, (KeyId, Option<ValueType>, KeyVersion, KeySource)>>,
    hashmap_publisher: Arc<pub_sub::PublisherManager<u64, (KeyId, SubKeyId, Option<ValueType>, KeyVersion, KeySource)>>,
    simple_get_queue: Arc<Mutex<HashMap<u64, Sender<Result<Option<(ValueType, KeyVersion, KeySource)>, SimpleKeyValueGetError>>>>>,
    hashmap_get_queue: Arc<Mutex<HashMap<u64, Sender<Result<Option<Vec<(SubKeyId, ValueType, KeyVersion, KeySource)>>, HashmapKeyValueGetError>>>>>,
    actions: Arc<RwLock<VecDeque<crate::KeyValueSdkEvent>>>,
}

impl KeyValueSdk {
    pub fn new() -> Self {
        Self {
            req_id_gen: Arc::new(AtomicU64::new(0)),
            uuid_gen: Arc::new(AtomicU64::new(0)),
            awaker: Arc::new(RwLock::new(None)),
            simple_publisher: Arc::new(pub_sub::PublisherManager::new()),
            hashmap_publisher: Arc::new(pub_sub::PublisherManager::new()),
            actions: Arc::new(RwLock::new(VecDeque::new())),
            simple_get_queue: Arc::new(Mutex::new(HashMap::new())),
            hashmap_get_queue: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set(&self, key: KeyId, value: Vec<u8>, ex: Option<u64>) {
        self.actions.write().push_back(crate::KeyValueSdkEvent::Set(key, value, ex));
        self.awaker.read().as_ref().unwrap().notify();
    }

    pub async fn get(&self, key: KeyId, timeout_ms: u64) -> Result<Option<(ValueType, KeyVersion, KeySource)>, SimpleKeyValueGetError> {
        let req_id = self.req_id_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.actions.write().push_back(crate::KeyValueSdkEvent::Get(req_id, key, timeout_ms));
        self.awaker.read().as_ref().unwrap().notify();
        let (tx, rx) = async_std::channel::bounded(1);
        self.simple_get_queue.lock().insert(req_id, tx);
        rx.recv().await.map_err(|_| SimpleKeyValueGetError::InternalError)?
    }

    pub fn del(&self, key: KeyId) {
        self.actions.write().push_back(crate::KeyValueSdkEvent::Del(key));
        self.awaker.read().as_ref().unwrap().notify();
    }

    pub fn subscribe(&self, key: KeyId, ex: Option<u64>) -> SimpleKeyValueSubscriber {
        let sub_uuid = self.uuid_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let actions = self.actions.clone();
        let awaker = self.awaker.clone();
        let subscriber = self.simple_publisher.subscribe(
            key,
            Box::new(move || {
                actions.write().push_back(crate::KeyValueSdkEvent::Unsub(sub_uuid, key));
                awaker.read().as_ref().unwrap().notify();
            }),
        );

        self.actions.write().push_back(crate::KeyValueSdkEvent::Sub(sub_uuid, key, ex));
        self.awaker.read().as_ref().unwrap().notify();

        subscriber
    }

    pub fn hset(&self, key: KeyId, sub_key: SubKeyId, value: Vec<u8>, ex: Option<u64>) {
        self.actions.write().push_back(crate::KeyValueSdkEvent::SetH(key, sub_key, value, ex));
        self.awaker.read().as_ref().unwrap().notify();
    }

    pub async fn hget(&self, key: KeyId, timeout_ms: u64) -> Result<Option<Vec<(SubKeyId, ValueType, KeyVersion, KeySource)>>, HashmapKeyValueGetError> {
        let req_id = self.req_id_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.actions.write().push_back(crate::KeyValueSdkEvent::GetH(req_id, key, timeout_ms));
        self.awaker.read().as_ref().unwrap().notify();
        let (tx, rx) = async_std::channel::bounded(1);
        self.hashmap_get_queue.lock().insert(req_id, tx);
        rx.recv().await.map_err(|_| HashmapKeyValueGetError::InternalError)?
    }

    pub fn hdel(&self, key: KeyId, sub_key: SubKeyId) {
        self.actions.write().push_back(crate::KeyValueSdkEvent::DelH(key, sub_key));
        self.awaker.read().as_ref().unwrap().notify();
    }

    pub fn hsubscribe(&self, key: u64, ex: Option<u64>) -> HashmapKeyValueSubscriber {
        let sub_uuid = self.uuid_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let actions = self.actions.clone();
        let awaker = self.awaker.clone();
        let subscriber = self.hashmap_publisher.subscribe(
            key,
            Box::new(move || {
                actions.write().push_back(crate::KeyValueSdkEvent::UnsubH(sub_uuid, key));
                awaker.read().as_ref().unwrap().notify();
            }),
        );

        self.actions.write().push_back(crate::KeyValueSdkEvent::SubH(sub_uuid, key, ex));
        self.awaker.read().as_ref().unwrap().notify();

        subscriber
    }

    pub fn hsubscribe_raw(&self, key: u64, uuid: u64, ex: Option<u64>, tx: Sender<(KeyId, SubKeyId, Option<ValueType>, KeyVersion, KeySource)>) {
        self.hashmap_publisher.sub_raw(key, uuid, tx);
        self.actions.write().push_back(crate::KeyValueSdkEvent::SubH(uuid, key, ex));
        self.awaker.read().as_ref().unwrap().notify();
    }

    pub fn hunsubscribe_raw(&self, key: u64, uuid: u64) {
        self.hashmap_publisher.unsub_raw(key, uuid);
        self.actions.write().push_back(crate::KeyValueSdkEvent::UnsubH(uuid, key));
        self.awaker.read().as_ref().unwrap().notify();
    }
}

impl ExternalControl for KeyValueSdk {
    fn set_awaker(&self, awaker: Arc<dyn Awaker>) {
        self.awaker.write().replace(awaker);
    }

    fn on_event(&self, event: KeyValueSdkEvent) {
        match event {
            KeyValueSdkEvent::OnKeyChanged(uuid, key, value, version, source) => {
                self.simple_publisher.publish(Some(uuid), key, (key, value, version, source));
            }
            KeyValueSdkEvent::OnKeyHChanged(uuid, key, sub_key, value, version, source) => {
                self.hashmap_publisher.publish(Some(uuid), key, (key, sub_key, value, version, source));
            }
            KeyValueSdkEvent::OnGet(req_id, key, res) => {
                if let Some(tx) = self.simple_get_queue.lock().remove(&req_id) {
                    if let Err(e) = tx.try_send(res.map_err(|e| match e {
                        KeyValueSdkEventError::NetworkError => SimpleKeyValueGetError::NetworkError,
                        KeyValueSdkEventError::Timeout => SimpleKeyValueGetError::Timeout,
                        KeyValueSdkEventError::InternalError => SimpleKeyValueGetError::InternalError,
                    })) {
                        log::error!("[KeyValueSdk] send get result request {req_id} for key {key} error: {:?}", e);
                    }
                }
            }
            KeyValueSdkEvent::OnGetH(req_id, key, res) => {
                if let Some(tx) = self.hashmap_get_queue.lock().remove(&req_id) {
                    if let Err(e) = tx.try_send(res.map_err(|e| match e {
                        KeyValueSdkEventError::NetworkError => HashmapKeyValueGetError::NetworkError,
                        KeyValueSdkEventError::Timeout => HashmapKeyValueGetError::Timeout,
                        KeyValueSdkEventError::InternalError => HashmapKeyValueGetError::InternalError,
                    })) {
                        log::error!("[KeyValueSdk] send get result request {req_id} for key {key} error: {:?}", e);
                    }
                }
            }
            _ => {}
        }
    }

    fn pop_action(&self) -> Option<KeyValueSdkEvent> {
        self.actions.write().pop_front()
    }
}

#[cfg(test)]
mod test {
    use std::{sync::Arc, time::Duration};

    use atm0s_sdn_utils::awaker::{Awaker, MockAwaker};

    use crate::{ExternalControl, KeyValueSdk, KeyValueSdkEvent};

    #[async_std::test]
    async fn sdk_get_should_fire_awaker_and_action() {
        let sdk = KeyValueSdk::new();
        let awaker = Arc::new(MockAwaker::default());

        sdk.set_awaker(awaker.clone());

        async_std::future::timeout(Duration::from_millis(100), sdk.get(1000, 100)).await.expect_err("Should timeout");
        assert_eq!(awaker.pop_awake_count(), 1);
        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Get(0, 1000, 100)));

        async_std::future::timeout(Duration::from_millis(100), sdk.hget(1000, 100)).await.expect_err("Should timeout");
        assert_eq!(awaker.pop_awake_count(), 1);
        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::GetH(1, 1000, 100)));
    }

    #[test]
    fn sdk_set_should_fire_awaker_and_action() {
        let sdk = KeyValueSdk::new();
        let awaker = Arc::new(MockAwaker::default());

        sdk.set_awaker(awaker.clone());

        sdk.set(1000, vec![1], Some(20000));
        assert_eq!(awaker.pop_awake_count(), 1);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Set(1000, vec![1], Some(20000))));

        sdk.del(1000);
        assert_eq!(awaker.pop_awake_count(), 1);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Del(1000)))
    }

    #[test]
    fn sdk_sub_should_fire_awaker_and_action() {
        let sdk = KeyValueSdk::new();
        let awaker = Arc::new(MockAwaker::default());

        sdk.set_awaker(awaker.clone());

        let handler1 = sdk.subscribe(1000, Some(20000));
        let handler2 = sdk.subscribe(1000, Some(20000));

        assert_eq!(awaker.pop_awake_count(), 2);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Sub(0, 1000, Some(20000))));
        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Sub(1, 1000, Some(20000))));
        assert_eq!(sdk.pop_action(), None);

        drop(handler1);
        drop(handler2);
        assert_eq!(awaker.pop_awake_count(), 2);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Unsub(0, 1000)));
        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::Unsub(1, 1000)));
        assert_eq!(sdk.pop_action(), None);
    }

    #[test]
    fn sdk_hset_should_fire_awaker_and_action() {
        let sdk = KeyValueSdk::new();
        let awaker = Arc::new(MockAwaker::default());

        sdk.set_awaker(awaker.clone());

        sdk.hset(1000, 11, vec![1], Some(20000));
        assert_eq!(awaker.pop_awake_count(), 1);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::SetH(1000, 11, vec![1], Some(20000))));
        assert_eq!(sdk.pop_action(), None);

        sdk.hdel(1000, 11);
        assert_eq!(awaker.pop_awake_count(), 1);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::DelH(1000, 11)));
        assert_eq!(sdk.pop_action(), None);
    }

    #[test]
    fn sdk_hsub_should_fire_awaker_and_action() {
        let sdk = KeyValueSdk::new();
        let awaker = Arc::new(MockAwaker::default());

        sdk.set_awaker(awaker.clone());

        let handler = sdk.hsubscribe(1000, Some(20000));
        assert_eq!(awaker.pop_awake_count(), 1);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::SubH(0, 1000, Some(20000))));
        assert_eq!(sdk.pop_action(), None);

        drop(handler);
        assert_eq!(awaker.pop_awake_count(), 1);

        assert_eq!(sdk.pop_action(), Some(KeyValueSdkEvent::UnsubH(0, 1000)));
        assert_eq!(sdk.pop_action(), None);
    }
}
