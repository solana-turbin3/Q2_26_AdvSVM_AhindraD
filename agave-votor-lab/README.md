# Votor-Jitter-Matrix

I pulled Votor's timing budget out of the Alpenglow whitepaper and built a unified harness that measures Agave's real `BankingStage` against it. I will also include a separate Tokio model so `tokio-console` can show what different task-jitter sources look like natively, without relying on `printf` debugging.

The architecture is deliberately split into two parts: a realworld OS threads and a synthetic async lab (Tokio).

## The problem

Extract the exact timing budget Votor requires, build a harness that measures Agave's real `Consumer` and `Committer` phases against it, and actively diagnose where jitter comes from under pressure (CPU scheduling, I/O stalls, and allocator pauses).

## Key Facts

* **`banking_stage`**: Agave's block-production stage. It receives verified transaction packets and turns them into recorded entries while a validator is leader.
* **`Consumer` / `Committer**`: The inner components of the `BankingStage` that actually process transaction batches, execute them against the SVM, and record them to PoH.
* **Votor**: Alpenglow's voting engine. For this assignment, the critical point is that block production *must* stay inside the slot budget so votes are not pushed toward timeout.
* **`tokio-console`**: A task-level tracing tool. It is phenomenal for Tokio async tasks, but it is blind to Agave's actual `BankingStage` workers because they run on native OS threads and use `crossbeam` channels.

## The Votor budget

| Symbol | Value | Meaning |
| --- | --- | --- |
| `δ` | ~`80 ms` | Actual network delay / one voting-round yardstick |
| `Δ` | ~`400 ms` | Conservative synchrony bound used for timeout sizing |
| `Δ_block` | `400 ms` | Block time / per-slot production budget |
| `Δ_timeout` | `3Δ = 1200 ms` | Timeout slack before skip voting |

The timeout formula is: `Timeout(i) = clock() + Δ_timeout + (i - s + 1) * Δ_block`

With a four-slot leader window, the harness prints these deadlines:

| Slot offset | Timeout from ParentReady |
| --- | --- |
| `slot + 0` | `1600 ms` |
| `slot + 1` | `2000 ms` |
| `slot + 2` | `2400 ms` |
| `slot + 3` | `2800 ms` |

The `Δ_timeout` is based on uppercase `Δ` (400ms), not lowercase `δ` (80ms). The real measurement target is much stricter than the final skip timeout: **steady-state block production should comfortably fit inside `Δ_block = 400 ms`.**

## My approach

The assignment asks for two things that cannot cleanly live in one process: observe real Agave `banking_stage` phase timings, and use `tokio-console` to trace async jitter.

### Part A: The Unified Agave Jitter Matrix (`agave-sys/`)

This is the real-system harness. Instead of treating `BankingStage` as a black box and measuring from outside the channels, it hooks directly into Agave's `Consumer` and `Committer`.

* **Active Interference:** It uses a `NoiseInjector` to spin up background threads that artificially thrash the CPU, block I/O, or lock the global allocator.
* **Phase Introspection:** It reads Agave's `LeaderExecuteAndCommitTimings` to record the exact microsecond durations of `load_execute`, `freeze_lock`, `record`, and `commit`.
* **Stability:** It stretches the genesis config (`ticks_per_slot *= 1024`) so the bank doesn't rotate mid-test, and explicitly prunes "Cycle 0" to isolate the cold-start penalty from the steady-state p99 metrics.

### Part B: The Async Jitter Lab (`tokio-sandbox/`)

This is a synthetic Tokio pipeline shaped like a small banking pipeline (producer -> scheduler -> workers). It injects the same jitter modes (CPU, IO, Allocator) but does it inside a pure Tokio runtime. This is the part you run with `tokio-console` to map the visual signatures of system bottlenecks.

## Running it

Because this harness bypasses standard RPC boundaries to instrument the SVM directly, it requires specific visibility modifications (`pub(crate)` -> `pub`) inside the Agave codebase.

To avoid applying these hacks manually, clone our locked and pre-configured repository directly next to this repo's parent directory:

```bash
git clone https://github.com/solana-turbin3/Q2_26_AdvSVM_AhindraD

```

**Run the Real Agave Jitter Matrix (Part A):**

Navigate to the harness directory and drive the matrix directly from your terminal using the built binary:

```bash
cd agave-votor-lab/

# Run the clean baseline
cargo run --bin real-banking-harness -- baseline

# Run the CPU starvation matrix
cargo run --bin real-banking-harness -- cpu

# Run the IO stall matrix
cargo run --bin real-banking-harness -- io

# Run the Allocator churn matrix
cargo run --bin real-banking-harness -- alloc

```

**Run the Tokio-console lab (Part B): [UNDER WORK FOR NOW]**



## Results & Key Findings

### 1. Real Agave Performance vs. Votor Budget

* **Massive Headroom:** The controlled Agave baseline easily fits inside the 400ms budget. The steady-state p99 total latency is extremely low (typically under 5ms), leaving over 390ms of headroom against the Votor deadline.
* **The Cold-Start Penalty:** By pruning Iteration 0, the matrix proves the existence of a massive cold-start penalty. Initial SVM program loading, cache warming, and allocator growth happen entirely in the first `load_execute` and `commit` phases.
* **Phase Isolation Under Stress:** * When the `NoiseInjector` applies *CPU Starvation*, the `load_execute` phase inflates the most, showing OS scheduler contention.
* When applying *Allocator Churn*, the `freeze_lock` and `load_execute` phases show aggressive p99 tail spikes, proving global allocator lock contention independent of channel overhead.




## Limitations

* The real harness uses small transfer batches. It strictly isolates the phase timings and compares controlled latency to the Votor budget, but it is not a full validator benchmark under heavy, conflicting account state load.