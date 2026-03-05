# Implementing Raft in Rust

Welcome! This repository contains my **from-scratch implementation** of the **Raft consensus algorithm** in **Rust**.

While I’m not new to **distributed systems**, this is my first deep dive into **Rust**. The goal of this project isn’t to create a production-ready Raft library—rather, it’s an opportunity for me to learn and explore Rust’s unique features while implementing Raft. Through this process, I’ll be tackling key topics like:

* Distributed consensus
* Failure handling and safety guarantees
* Concurrency and ownership in Rust
* Correctness-driven development
* Testing distributed systems

I’m excited to learn how Rust approaches these concepts, particularly when it comes to memory safety and concurrency. I’ll be experimenting with different Rust features as I work through this, and I hope you find it as interesting and educational as I do.

## Specification Reference

The implementation follows the original Raft dissertation by Diego Ongaro:

* Diego Ongaro, *In Search of an Understandable Consensus Algorithm*
  [Link to the dissertation](https://github.com/ongardie/dissertation)

## Testing Strategy

Testing is a big focus of this project. Rather than relying on traditional unit tests, I’m taking a more comprehensive approach to ensure correctness, even under edge cases and failures:

* **Deterministic simulations** of node clusters
* **Failure injection**: Crashes, partitions, and message reordering
* **Invariant verification** after every state transition
* **Safety property testing** under randomized schedules

I’m making sure the implementation is robust enough to handle the kinds of challenges real-world systems face, and learning how to approach testing in distributed systems along the way.
