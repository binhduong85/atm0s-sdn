#[cfg(test)]
mod tests {
    use async_std::prelude::FutureExt;
    use async_std::task::JoinHandle;
    use atm0s_sdn::SharedRouter;
    use atm0s_sdn::{convert_enum, NetworkPlane, NetworkPlaneConfig};
    use atm0s_sdn::{FeedbackType, NodeAddr, NodeAddrBuilder, NodeId, NumberInfo, PubsubSdk, PubsubServiceBehaviour, PubsubServiceBehaviourEvent, PubsubServiceHandlerEvent};
    use atm0s_sdn::{KeyValueBehavior, KeyValueBehaviorEvent, KeyValueHandlerEvent, KeyValueSdk, KeyValueSdkEvent};
    use atm0s_sdn::{LayersSpreadRouterSyncBehavior, LayersSpreadRouterSyncBehaviorEvent, LayersSpreadRouterSyncHandlerEvent};
    use atm0s_sdn::{ManualBehavior, ManualBehaviorConf, ManualBehaviorEvent, ManualHandlerEvent};
    use atm0s_sdn::{OptionUtils, SystemTimer};
    use atm0s_sdn_transport_vnet::VnetEarth;
    use bytes::Bytes;
    use std::{sync::Arc, time::Duration, vec};

    #[derive(convert_enum::From, convert_enum::TryInto)]
    enum ImplBehaviorEvent {
        Pubsub(PubsubServiceBehaviourEvent),
        KeyValue(KeyValueBehaviorEvent),
        RouterSync(LayersSpreadRouterSyncBehaviorEvent),
        Manual(ManualBehaviorEvent),
    }

    #[derive(convert_enum::From, convert_enum::TryInto)]
    enum ImplHandlerEvent {
        Pubsub(PubsubServiceHandlerEvent),
        KeyValue(KeyValueHandlerEvent),
        RouterSync(LayersSpreadRouterSyncHandlerEvent),
        Manual(ManualHandlerEvent),
    }

    #[derive(convert_enum::From, convert_enum::TryInto)]
    enum ImplSdkEvent {
        KeyValue(KeyValueSdkEvent),
    }

    async fn run_node(vnet: Arc<VnetEarth>, node_id: NodeId, seeds: Vec<NodeAddr>) -> (PubsubSdk, NodeAddr, JoinHandle<()>) {
        log::info!("Run node {} connect to {:?}", node_id, seeds);
        let node_addr = Arc::new(NodeAddrBuilder::new(node_id));
        let transport = Box::new(atm0s_sdn_transport_vnet::VnetTransport::new(vnet, node_addr.addr()));
        let timer = Arc::new(SystemTimer());

        let router = SharedRouter::new(node_id);
        let manual = ManualBehavior::new(ManualBehaviorConf {
            node_id,
            node_addr: node_addr.addr(),
            seeds,
            local_tags: vec![],
            connect_tags: vec![],
        });

        let router_sync_behaviour = LayersSpreadRouterSyncBehavior::new(router.clone());
        let kv_sdk = KeyValueSdk::new();
        let kv_behaviour = KeyValueBehavior::new(node_id, 3000, Some(Box::new(kv_sdk.clone())));
        let (pubsub_behavior, pubsub_sdk) = PubsubServiceBehaviour::new(node_id, timer.clone());

        let mut plane = NetworkPlane::<ImplBehaviorEvent, ImplHandlerEvent, ImplSdkEvent>::new(NetworkPlaneConfig {
            node_id,
            tick_ms: 100,
            behaviors: vec![Box::new(pubsub_behavior), Box::new(kv_behaviour), Box::new(router_sync_behaviour), Box::new(manual)],
            transport,
            timer,
            router: Arc::new(router.clone()),
        });

        let join = async_std::task::spawn(async move {
            plane.started();
            while let Ok(_) = plane.recv().await {}
            plane.stopped();
        });

        (pubsub_sdk, node_addr.addr(), join)
    }

    /// Testing local pubsub
    #[async_std::test]
    async fn local_node_single() {
        let vnet = Arc::new(VnetEarth::default());
        let (sdk, _addr, join) = run_node(vnet, 1, vec![]).await;

        async_std::task::sleep(Duration::from_millis(300)).await;

        let producer = sdk.create_publisher(1111);
        let consumer = sdk.create_consumer_single(producer.identify(), Some(10));

        let data = Bytes::from(vec![1, 2, 3, 4]);
        producer.send(data.clone());
        let got_value = consumer.recv().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_value, (consumer.uuid(), 1, 1111, data));

        const PASS_FEEDBACK_TYPE_ID: u8 = 2;
        consumer.feedback(PASS_FEEDBACK_TYPE_ID, FeedbackType::Passthrough(vec![1]));
        let got_feedback = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback.channel, producer.identify());
        assert_eq!(got_feedback.id, PASS_FEEDBACK_TYPE_ID);
        assert_eq!(got_feedback.feedback_type, FeedbackType::Passthrough(vec![1]));

        const NUMBER_FEEDBACK_TYPE_ID: u8 = 3;
        consumer.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 },
            },
        );
        let got_feedback1 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback1.channel, producer.identify());
        assert_eq!(got_feedback1.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback1.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 }
            }
        );

        let consumer2 = sdk.create_consumer_single(producer.identify(), Some(10));
        consumer2.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 2, min: 2, sum: 2 },
            },
        );
        let got_feedback2 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback2.channel, producer.identify());
        assert_eq!(got_feedback2.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback2.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 2, max: 2, min: 1, sum: 3 }
            }
        );

        join.cancel().await.print_none("Should cancel join");
    }

    /// Testing local pubsub
    #[async_std::test]
    async fn local_node_auto() {
        let vnet = Arc::new(VnetEarth::default());
        let (sdk, _addr, join) = run_node(vnet, 1, vec![]).await;

        async_std::task::sleep(Duration::from_millis(300)).await;

        log::info!("create publisher");
        let producer = sdk.create_publisher(1111);
        log::info!("create consumer");
        let consumer = sdk.create_consumer(1111, Some(10));

        async_std::task::sleep(Duration::from_millis(300)).await;

        let data = Bytes::from(vec![1, 2, 3, 4]);
        producer.send(data.clone());
        let got_value = consumer.recv().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_value, (consumer.uuid(), 1, 1111, data));

        const PASS_FEEDBACK_TYPE_ID: u8 = 2;
        consumer.feedback(PASS_FEEDBACK_TYPE_ID, FeedbackType::Passthrough(vec![1]));
        let got_feedback = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback.channel, producer.identify());
        assert_eq!(got_feedback.id, PASS_FEEDBACK_TYPE_ID);
        assert_eq!(got_feedback.feedback_type, FeedbackType::Passthrough(vec![1]));

        const NUMBER_FEEDBACK_TYPE_ID: u8 = 3;
        consumer.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 },
            },
        );
        let got_feedback1 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback1.channel, producer.identify());
        assert_eq!(got_feedback1.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback1.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 }
            }
        );

        let consumer2 = sdk.create_consumer_single(producer.identify(), Some(10));
        consumer2.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 2, min: 2, sum: 2 },
            },
        );
        let got_feedback2 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback2.channel, producer.identify());
        assert_eq!(got_feedback2.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback2.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 2, max: 2, min: 1, sum: 3 }
            }
        );

        join.cancel().await.print_none("Should cancel join");
    }

    /// Testing remote
    #[async_std::test]
    async fn remote_node_single() {
        let vnet = Arc::new(VnetEarth::default());
        let (sdk1, addr1, join1) = run_node(vnet.clone(), 1, vec![]).await;
        let (sdk2, _addr2, join2) = run_node(vnet, 2, vec![addr1]).await;

        async_std::task::sleep(Duration::from_millis(300)).await;

        let producer = sdk1.create_publisher(1111);
        let consumer = sdk2.create_consumer_single(producer.identify(), Some(10));

        async_std::task::sleep(Duration::from_millis(300)).await;

        let data = Bytes::from(vec![1, 2, 3, 4]);
        producer.send(data.clone());
        let got_value = consumer.recv().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_value, (consumer.uuid(), 1, 1111, data));

        const PASS_FEEDBACK_TYPE_ID: u8 = 2;
        consumer.feedback(PASS_FEEDBACK_TYPE_ID, FeedbackType::Passthrough(vec![1]));
        let got_feedback = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback.channel, producer.identify());
        assert_eq!(got_feedback.id, PASS_FEEDBACK_TYPE_ID);
        assert_eq!(got_feedback.feedback_type, FeedbackType::Passthrough(vec![1]));

        const NUMBER_FEEDBACK_TYPE_ID: u8 = 3;
        consumer.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 },
            },
        );
        let got_feedback1 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback1.channel, producer.identify());
        assert_eq!(got_feedback1.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback1.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 }
            }
        );

        let consumer2 = sdk2.create_consumer_single(producer.identify(), Some(10));
        consumer2.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 2, min: 2, sum: 2 },
            },
        );
        let got_feedback2 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback2.channel, producer.identify());
        assert_eq!(got_feedback2.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback2.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 2, max: 2, min: 1, sum: 3 }
            }
        );

        join1.cancel().await.print_none("Should cancel join");
        join2.cancel().await.print_none("Should cancel join");
    }

    /// Testing remote
    #[async_std::test]
    async fn remote_node_auto() {
        let vnet = Arc::new(VnetEarth::default());
        let (sdk1, addr1, join1) = run_node(vnet.clone(), 1, vec![]).await;
        let (sdk2, _addr2, join2) = run_node(vnet, 2, vec![addr1]).await;

        async_std::task::sleep(Duration::from_millis(300)).await;

        let producer = sdk1.create_publisher(1111);
        let consumer = sdk2.create_consumer(1111, Some(10));

        async_std::task::sleep(Duration::from_millis(300)).await;

        let data = Bytes::from(vec![1, 2, 3, 4]);
        producer.send(data.clone());
        let got_value = consumer.recv().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_value, (consumer.uuid(), 1, 1111, data));

        const PASS_FEEDBACK_TYPE_ID: u8 = 2;
        consumer.feedback(PASS_FEEDBACK_TYPE_ID, FeedbackType::Passthrough(vec![1]));
        let got_feedback = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback.channel, producer.identify());
        assert_eq!(got_feedback.id, PASS_FEEDBACK_TYPE_ID);
        assert_eq!(got_feedback.feedback_type, FeedbackType::Passthrough(vec![1]));

        const NUMBER_FEEDBACK_TYPE_ID: u8 = 3;
        consumer.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 },
            },
        );
        let got_feedback1 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback1.channel, producer.identify());
        assert_eq!(got_feedback1.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback1.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 1, min: 1, sum: 1 }
            }
        );

        let consumer2 = sdk2.create_consumer_single(producer.identify(), Some(10));
        consumer2.feedback(
            NUMBER_FEEDBACK_TYPE_ID,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 1, max: 2, min: 2, sum: 2 },
            },
        );
        let got_feedback2 = producer.recv_feedback().timeout(Duration::from_secs(1)).await.expect("Should get success").expect("Should some");
        assert_eq!(got_feedback2.channel, producer.identify());
        assert_eq!(got_feedback2.id, NUMBER_FEEDBACK_TYPE_ID);
        assert_eq!(
            got_feedback2.feedback_type,
            FeedbackType::Number {
                window_ms: 200,
                info: NumberInfo { count: 2, max: 2, min: 1, sum: 3 }
            }
        );

        join1.cancel().await.print_none("Should cancel join");
        join2.cancel().await.print_none("Should cancel join");
    }
}
