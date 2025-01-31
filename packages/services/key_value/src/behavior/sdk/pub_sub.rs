use std::{
    collections::HashMap,
    fmt::Debug,
    hash::Hash,
    sync::{atomic::AtomicU64, Arc},
};

use async_std::channel::{Receiver, Sender};
use atm0s_sdn_utils::error_handle::ErrorUtils;
use parking_lot::RwLock;

struct SubscribeContainer<T> {
    subscribers: HashMap<u64, (Sender<T>, Box<dyn FnOnce() + Send + Sync>)>,
}

pub struct PublisherManager<K, T> {
    uuid: AtomicU64,
    subscribers: Arc<RwLock<HashMap<K, SubscribeContainer<T>>>>,
}

impl<K: Debug + Hash + Eq + Copy, T: Clone> PublisherManager<K, T> {
    pub fn new() -> Self {
        Self {
            uuid: AtomicU64::new(0),
            subscribers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn sub_raw(&self, key: K, uuid: u64, tx: Sender<T>) {
        let mut subscribers = self.subscribers.write();
        match subscribers.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().subscribers.insert(uuid, (tx, Box::new(|| {})));
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let clear_handler: Box<dyn FnOnce() + Send + Sync> = Box::new(|| {});
                entry.insert(SubscribeContainer {
                    subscribers: HashMap::from([(uuid, (tx, clear_handler))]),
                });
            }
        }
    }

    pub fn unsub_raw(&self, key: K, uuid: u64) {
        let mut subscribers = self.subscribers.write();
        if let Some(container) = subscribers.get_mut(&key) {
            if let Some((_, clear_handler)) = container.subscribers.remove(&uuid) {
                clear_handler();
            }
            if container.subscribers.is_empty() {
                subscribers.remove(&key);
            }
        }
    }

    /// subscribe and return Subscriber and is_new
    /// is_new is true if this is the first subscriber
    /// is_new is false if this is not the first subscriber
    /// If Subscriber is drop, it automatically unsubscribe
    pub fn subscribe(&self, key: K, clear_handler: Box<dyn FnOnce() + Send + Sync>) -> Subscriber<K, T> {
        let mut subscribers = self.subscribers.write();
        let uuid = self.uuid.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx) = async_std::channel::unbounded();
        match subscribers.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().subscribers.insert(uuid, (tx, clear_handler));
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(SubscribeContainer {
                    subscribers: HashMap::from([(uuid, (tx, clear_handler))]),
                });
            }
        };

        Subscriber {
            uuid,
            key,
            subscribers: self.subscribers.clone(),
            rx,
        }
    }

    /// publish event to specific subscriber if uuid is Some, if None publish to all subscribers of key
    pub fn publish(&self, uuid: Option<u64>, key: K, data: T) {
        log::info!("publish event {:?} {:?}", uuid, key);
        let subscribers = self.subscribers.read();
        if let Some(container) = subscribers.get(&key) {
            if let Some(uuid) = uuid {
                if let Some((tx, _)) = container.subscribers.get(&uuid) {
                    tx.send_blocking(data.clone()).print_error("Should send event");
                }
            } else {
                for (_, (tx, _)) in container.subscribers.iter() {
                    tx.send_blocking(data.clone()).print_error("Should send event");
                }
            }
        }
    }
}

pub struct Subscriber<K: Hash + Eq + Copy, T> {
    uuid: u64,
    key: K,
    subscribers: Arc<RwLock<HashMap<K, SubscribeContainer<T>>>>,
    rx: Receiver<T>,
}

impl<K: Hash + Eq + Copy, T> Subscriber<K, T> {
    pub async fn recv(&mut self) -> Option<T> {
        self.rx.recv().await.ok()
    }
}

impl<K: Hash + Eq + Copy, T> Drop for Subscriber<K, T> {
    fn drop(&mut self) {
        let mut subscribers = self.subscribers.write();
        let container = subscribers.get_mut(&self.key).expect("Should have subscribers");
        if let Some((_, clear_handler)) = container.subscribers.remove(&self.uuid) {
            clear_handler();
        }
        if container.subscribers.is_empty() {
            subscribers.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicU8, Arc};

    #[test]
    fn test_simple_pubsub() {
        let pub_manager = super::PublisherManager::<u64, u64>::new();

        {
            let destroy_count = Arc::new(AtomicU8::new(0));
            let destroy_count_c = destroy_count.clone();
            let mut sub1 = pub_manager.subscribe(
                1,
                Box::new(move || {
                    destroy_count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }),
            );

            let mut sub2 = pub_manager.subscribe(1, Box::new(|| {}));
            let destroy_count_c = destroy_count.clone();
            let mut sub3 = pub_manager.subscribe(
                2,
                Box::new(move || {
                    destroy_count_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }),
            );

            pub_manager.publish(None, 1, 1);
            pub_manager.publish(None, 2, 2);

            // sub1 should receive 1 with timeout 1s
            assert_eq!(async_std::task::block_on(sub1.recv()), Some(1));
            // sub2 should receive 1 with timeout 1s
            assert_eq!(async_std::task::block_on(sub2.recv()), Some(1));
            // sub3 should receive 2 with timeout 1s
            assert_eq!(async_std::task::block_on(sub3.recv()), Some(2));

            // drop sub1
            drop(sub1);
            // drop sub2
            drop(sub2);
            // drop sub3
            drop(sub3);

            // destroy_count should be 2
            assert_eq!(destroy_count.load(std::sync::atomic::Ordering::SeqCst), 2);
        }

        //after drop subscribers should be empty
        assert!(pub_manager.subscribers.read().is_empty());
    }

    #[test]
    fn test_simple_pubsub_memory() {
        let pub_manager = super::PublisherManager::<u64, u64>::new();

        let info = allocation_counter::measure(|| {
            let sub1 = pub_manager.subscribe(1, Box::new(|| {}));
            let sub2 = pub_manager.subscribe(1, Box::new(|| {}));
            let sub3 = pub_manager.subscribe(2, Box::new(|| {}));

            // drop sub1
            drop(sub1);
            // drop sub2
            drop(sub2);
            // drop sub3
            drop(sub3);

            // shrink to fit for check memory leak
            pub_manager.subscribers.write().shrink_to_fit();

            //after drop subscribers should be empty
            assert!(pub_manager.subscribers.read().is_empty());
        });
        assert_eq!(info.count_current, 0);
    }
}
