# Agave Votor Budget & Jitter Harness

This PoW extracts the exact Votor per-round timing budget from the Alpenglow whitepaper and measures Agave's real `BankingStage` against it. It also models asynchronous task jitter using `tokio-console` to map specific bottleneck signatures.

## The Votor Budget

The Alpenglow whitepaper uses two different symbols for delay, which creates a common mathematical trap when sizing the protocol timeouts. The timeout is built on the conservative bound ($\Delta$), not the actual network delay ($\delta$).

- **$\delta$ (Network Delay):** ~80ms (The per-batch yardstick).
- **$\Delta$ (Conservative Bound):** ~400ms.
- **$\Delta_{block}$ (Block Production Budget):** 400ms.
- **$\Delta_{timeout}$ (Skip-vote timeout):** $3\Delta = 1200ms$.
- **First Slot Timeout:** $1600ms$ ($\Delta_{timeout} + \Delta_{block}$).

**The Goal:** Agave's `BankingStage` must comfortably process batches and record entries within the 400ms $\Delta_{block}$ budget.

## Architecture & Approach

The assignment requires observing the real Agave `banking_stage` while using `tokio-console` to trace jitter. However, these two requirements cannot live in the same diagnostic program: Agave's real `BankingStage` workers run on native OS threads communicating via synchronous `crossbeam` channels, whereas `tokio-console` can only trace Tokio async tasks.

To solve this, the harness is deliberately split into two distinct parts:

### 1. Part A: Real System Baseline (`real-banking-harness/`)

A wall-clock measurement tool built against the real `solana_core::banking_stage::BankingStage`. It measures the end-to-end latency (send-to-record) and the per-phase microsecond breakdown (`load_execute`, `freeze_lock`, `record`, `commit`).

### 2. Part B: Tokio Jitter Model (`async-jitter-model/`)

A synthetic Tokio async pipeline shaped exactly like `banking_stage` (producer $\rightarrow$ scheduler $\rightarrow$ workers). It injects specific system faults (CPU pressure, I/O stalls, allocator churn) so `tokio-console` can map their visual signatures without relying on `printf` debugging.

## Key Findings

### 1. Real Agave Performance vs. Votor Budget

- **Massive Headroom:** The controlled Agave baseline easily fits inside the 400ms budget. Median steady-state execution takes roughly 150µs to 2ms, with a p99 of ~12ms. This leaves over 380ms of headroom against the Votor deadline.
- **The Cold-Start Penalty:** There is a distinct latency spike on iteration 0 (up to ~10ms). The per-phase breakdown proves this penalty happens entirely in the `load_execute` and `commit` phases due to lazy SVM program loading, cold caches, and allocator growth.
- **Stress Resilience:** Even under injected system stress, the real `BankingStage` p99 tail latency only grew to 66-87ms, still safely below the 400ms budget.

### 2. Async Jitter Signatures (`tokio-console`)

By observing the synthetic async model, we mapped exactly what system bottlenecks look like in the Tokio runtime:

- **CPU Contention:** Shows up as **high schedule delay but flat execution time**. The task is ready but waiting in the queue because no threads are free.
- **I/O Stalls:** Shows up as a **massive spike in the execution tail latency**. A synchronous blocking call freezes the runtime thread, stalling its neighbors.
- **Allocator Churn:** Shows up as a **rising median execute time**. The thread is doing real busy work allocating memory.

## How to Run

**Run the Real Agave Harness (Part A):**

```bash
# Run the baseline latency measurement
cargo run --bin real-banking-harness
```

**Diagnose Async Jitter (Part B):**

```bash
# Run the model in a specific stress mode (cpu, io, or alloc)
cargo run --bin async-jitter-model -- cpu

# In a separate terminal, attach the console to view the jitter signatures
tokio-console
```

## Limitations

- The real harness currently uses single-transfer batches, meaning it bypasses real account-conflict scheduling where lock contention would typically appear.
- Part B is a structural model of Agave's pipeline built on Tokio; its absolute execution times are illustrative, as its purpose is purely to capture the visual shapes of scheduling delays.
