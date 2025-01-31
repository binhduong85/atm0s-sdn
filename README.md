<p align="center">
 <a href="https://github.com/8xFF/atm0s-sdn/actions">
  <img src="https://github.com/8xFF/atm0s-sdn/actions/workflows/rust.yml/badge.svg?branch=master">
 </a>
 <a href="https://codecov.io/gh/8xff/atm0s-sdn">
  <img src="https://codecov.io/gh/8xff/atm0s-sdn/branch/master/graph/badge.svg">
 </a>
 <a href="https://app.codacy.com/gh/8xFF/atm0s-sdn/dashboard?utm_source=gh&utm_medium=referral&utm_content=&utm_campaign=Badge_grade">
  <img src="https://app.codacy.com/project/badge/Grade/c8d34f07fe5f417084aac8d8a1ef3857"/>
 </a>
 <a href="https://deps.rs/repo/github/8xff/atm0s-sdn">
  <img src="https://deps.rs/repo/github/8xff/atm0s-sdn/status.svg">
 </a>
<!--  <a href="https://crates.io/crates/8xff-sdn">
  <img src="https://img.shields.io/crates/v/8xff-sdn.svg">
 </a> -->
<!--  <a href="https://docs.rs/8xff-sdn">
  <img src="https://docs.rs/8xff-sdn/badge.svg">
 </a> -->
 <a href="https://github.com/8xFF/atm0s-sdn/blob/master/LICENSE">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT">
 </a>
 <a href="https://discord.gg/tJ6dxBRk">
  <img src="https://img.shields.io/discord/1173844241542287482?logo=discord" alt="Discord">
 </a>
</p>

# Atm0s SDN: Global-scaled Ultra-low latency Decentralized SDN

A SAN I/O driven, open-source decentralized network infrastructure that can deliver high-quality data with minimal latency and efficient cost, similar to what Cloudflare achieves for their network.

For a deep dive into the technical aspects of network architecture, please refer to our [Smart-Routing](https://github.com/8xFF/atm0s-sdn/blob/master/docs/smart_routing.md)

## Features

  - Blazingly fast, powered by Rust.
  - High availability by being fully distributed, with no central controller.
  - Multi-zone support, high scalability.
  - Definable Metric based Adaptive routing: cost, latency, .etc...
  - Fixed size routing table.
  - Designed with large scale built-in PubSub service.
  - Automatic Network orchestration and discovery (also can be manual).
  - High extendibility by using Network Service.
  - Built-in features: PubSub, KeyValue, VPN.
  - Cross platform: Linux, MacOs, Windows.

## Architecture

Each node in the network is embedded with Geo-Location data inside its ID. A Node ID consists of multiple layers, and every node will have multiple routing tables, each corresponding to a layer.

  - Layer1: Geo1 Table (Zone level)
  - Layer2: Geo2 Table (Country level)
  - Layer3: Inner Geo Group Table (City level)
  - Layer4: Inner Group Index Table (DC level)

TODO: Graphics instead of bulletlist
TODO: ARCHITECTURE.md with general information about: Project, System structure, Design philosophy, ...
## Getting started

```bash
cargo add 8xff-sdn
```

### Create a group chat application (Optional)
You can refer to [the chat_example file here](examples/examples/chat_example.rs) to get started on the basis of building an application for the network.

### Demo group chat application
#### Running manual discovery multi nodes in a single device

Start node1:

```bash
cargo run --example chat_example -- --node-id 0
```

Start node2:

```bash
cargo run --example chat_example --node-id 1 --neighbour /p2p/0/ip4/127.0.0.1/udp/50000
```

In node1

```shell
> router
[Registry 0] local services: [] remote services: [], nexts []
[Table 0/3/0] slots: []
[Table 0/2/0] slots: []
[Table 0/1/0] slots: []
[Table 0/0/0] slots: [1]
# Join the room using join command with room id
> join 1
```

In node2

```shell
> join 1
> send hello
```

Now, node1 will receive a message from node2

```shell
Node 1 to room 1: hello
```

Available commands:
  - `help`: Show available commands and description
  - `router`: Print routing table
  - `join`: Join a room
  - `send`: Send a message to room
  - `leave`: Leave joined room

#### Running manual discovery multi nodes in multi devices

It can also start chat-example in multi nodes and connect over LAN or Internet

Start node1:

```bash
cargo run --example chat_example --node-id 0
```

Start node2:

```bash
cargo run --example chat_example --node-id 1 --neighbours /p2p/0/ip4/[IP_HERE]/udp/50000
```


## Showcases

  - Media Server: [Repo](https://github.com/8xFF/decentralized-media-server)
  - VPN: [Repo](https://github.com/8xFF/atm0s-sdn/tree/master/packages/services/tun_tap)
  - MiniRedis: [Repo](https://github.com/8xFF/atm0s-sdn/tree/master/packages/apps/redis)

## Contributing
The project is continuously being improved and updated. We are always looking for ways to make it better, whether that's through optimizing performance, adding new features, or fixing bugs. We welcome contributions from the community and are always looking for new ideas and suggestions. If you find it interesting or believe it could be helpful, we welcome your contributions to the codebase or consider starring the repository to show your support and motivate our team!

For more information, you can join our [Discord channel](https://discord.gg/tJ6dxBRk)


## Roadmap

The first version will be released together with [Media Server](https://github.com/8xFF/decentralized-media-server) at the end of 2023.

Details on our roadmap can be seen [TBA]().

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

We would like to thank all the contributors who have helped in making this project successful.
