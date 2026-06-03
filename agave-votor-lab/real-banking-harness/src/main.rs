//! Phase-Introspection Jitter Matrix (Binary Executable)
//! 
//! This harness merges active hardware interference (CPU/IO/Alloc stress) with 
//! direct introspection of Agave's `Consumer` and `Committer`. It isolates exactly 
//! which phase of the `BankingStage` (load, freeze, record, commit) violates 
//! the Votor Δ_block budget under stress.

use {
    crossbeam_channel::unbounded,
    hdrhistogram::Histogram,
    solana_core::banking_stage::{committer::Committer, consumer::Consumer},
    solana_ledger::genesis_utils::{
        bootstrap_validator_stake_lamports, create_genesis_config_with_leader,
        GenesisConfigInfo,
    },
    solana_poh::{
        record_channels::record_channels, transaction_recorder::TransactionRecorder,
    },
    solana_runtime::bank::Bank,
    solana_runtime_transaction::runtime_transaction::RuntimeTransaction,
    solana_system_transaction as system_transaction,
    std::{
        env,
        hint::black_box,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread::{self, sleep, JoinHandle},
        time::{Duration, Instant},
    },
};

const VOTOR_DELTA_BLOCK_US: u64 = 400_000; // 400ms in microseconds
const N_ITERATIONS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StressProfile {
    Baseline,
    CpuStarvation,
    IoBlocking,
    AllocatorChurn,
}

/// Background engine that actively fights the Agave workers for hardware resources.
struct NoiseInjector {
    halt_signal: Arc<AtomicBool>,
    worker_threads: Vec<JoinHandle<()>>,
}

impl NoiseInjector {
    fn ignite(profile: StressProfile) -> Self {
        let halt_signal = Arc::new(AtomicBool::new(false));
        let mut worker_threads = Vec::new();

        if profile == StressProfile::CpuStarvation {
            let cores_to_hog = thread::available_parallelism()
                .map(|n| n.get().saturating_sub(1).max(1))
                .unwrap_or(2)
                .min(8);

            for id in 0..cores_to_hog {
                let halt_clone = halt_signal.clone();
                worker_threads.push(
                    thread::Builder::new()
                        .name(format!("noise-cpu-{}", id))
                        .spawn(move || {
                            let mut entropy = id as u64 + 1;
                            while !halt_clone.load(Ordering::Relaxed) {
                                for _ in 0..1_000_000 {
                                    entropy = entropy.rotate_left(11).wrapping_mul(6364136223846793005);
                                    black_box(entropy);
                                }
                                thread::yield_now();
                            }
                        })
                        .unwrap(),
                );
            }
        }
        Self { halt_signal, worker_threads }
    }

    fn trigger_pre_send_interference(&self, profile: StressProfile, cycle: usize) {
        match profile {
            StressProfile::Baseline | StressProfile::CpuStarvation => {}
            StressProfile::IoBlocking if cycle % 4 == 0 => {
                // Simulate an external synchronous I/O stall on the producer side
                sleep(Duration::from_millis(15));
            }
            StressProfile::AllocatorChurn => {
                // Force the global allocator to thrash
                let mut memory_block = vec![0_u8; 32 * 1024 * 1024];
                for (idx, byte) in memory_block.iter_mut().enumerate().step_by(4096) {
                    *byte = idx.wrapping_add(cycle) as u8;
                }
                black_box(memory_block);
            }
            StressProfile::IoBlocking => {}
        }
    }
}

impl Drop for NoiseInjector {
    fn drop(&mut self) {
        self.halt_signal.store(true, Ordering::Relaxed);
        for thread in self.worker_threads.drain(..) {
            thread.join().unwrap();
        }
    }
}

struct PhaseHistograms {
    load_execute: Histogram<u64>,
    freeze_lock: Histogram<u64>,
    record: Histogram<u64>,
    commit: Histogram<u64>,
    total: Histogram<u64>,
}

impl PhaseHistograms {
    fn new() -> Self {
        Self {
            load_execute: Histogram::<u64>::new(3).unwrap(),
            freeze_lock: Histogram::<u64>::new(3).unwrap(),
            record: Histogram::<u64>::new(3).unwrap(),
            commit: Histogram::<u64>::new(3).unwrap(),
            total: Histogram::<u64>::new(3).unwrap(),
        }
    }
}

/// Stretches the slot time so Agave doesn't rotate the bank mid-test,
/// preventing artificial latency spikes in our data.
fn build_stretched_genesis_for_stability(lamports: u64) -> GenesisConfigInfo {
    let validator_pubkey = solana_pubkey::new_rand();
    let mut info = create_genesis_config_with_leader(
        lamports,
        &validator_pubkey,
        bootstrap_validator_stake_lamports(),
    );
    info.genesis_config.ticks_per_slot *= 1024;
    info
}

fn execute_matrix_profile(profile: StressProfile) {
    let GenesisConfigInfo {
        genesis_config,
        mint_keypair,
        ..
    } = build_stretched_genesis_for_stability(1_000_000_000);
    let (bank, _bank_forks) = Bank::new_with_bank_forks_for_tests(&genesis_config);
    let start_hash = bank.last_blockhash();

    let (record_sender, mut record_receiver) = record_channels(false);
    let recorder = TransactionRecorder::new(record_sender);
    record_receiver.restart(bank.bank_id());
    
    let (replay_vote_sender, _replay_vote_receiver) = unbounded();
    let committer = Committer::new(None, replay_vote_sender, None);
    let consumer = Consumer::new(committer, recorder, None);

    let noise_engine = NoiseInjector::ignite(profile);
    let mut metrics = PhaseHistograms::new();

    for cycle in 0..N_ITERATIONS {
        noise_engine.trigger_pre_send_interference(profile, cycle);

        let recipient = solana_pubkey::new_rand();
        let tx = system_transaction::transfer(&mint_keypair, &recipient, 1, start_hash);
        let sanitized = vec![RuntimeTransaction::from_transaction_for_tests(tx)];

        let t0 = Instant::now();
        let output = consumer.process_and_record_transactions(&bank, &sanitized);
        let total_time_us = t0.elapsed().as_micros() as u64;

        let exec_result = output.execute_and_commit_transactions_output;
        assert!(
            exec_result.commit_transactions_result.is_ok(),
            "Matrix failed: transaction did not commit cleanly."
        );

        let timings = exec_result.execute_and_commit_timings;

        // Skip the cold-start (cycle 0) from the histogram to maintain steady-state purity
        if cycle > 0 {
            metrics.load_execute.record(timings.load_execute_us).unwrap();
            metrics.freeze_lock.record(timings.freeze_lock_us).unwrap();
            metrics.record.record(timings.record_us).unwrap();
            metrics.commit.record(timings.commit_us).unwrap();
            metrics.total.record(total_time_us).unwrap();
        }
    }

    print_cli_report(profile, &metrics);
}

fn print_cli_report(profile: StressProfile, metrics: &PhaseHistograms) {
    println!("\n========================================================");
    println!(" AGAVE PHASE JITTER MATRIX: {:?}", profile);
    println!("========================================================");
    println!(" Metrics tracked in microseconds (us) | N = {}", N_ITERATIONS - 1);
    println!("--------------------------------------------------------");
    println!(" {:<15} | {:<8} | {:<8} | {:<8} | {:<8}", "Phase", "p50", "p90", "p99", "Max");
    println!("--------------------------------------------------------");
    
    let print_row = |name: &str, hist: &Histogram<u64>| {
        println!(
            " {:<15} | {:<8} | {:<8} | {:<8} | {:<8}",
            name,
            hist.value_at_quantile(0.50),
            hist.value_at_quantile(0.90),
            hist.value_at_quantile(0.99),
            hist.max()
        );
    };

    print_row("Load & Execute", &metrics.load_execute);
    print_row("Freeze Lock", &metrics.freeze_lock);
    print_row("Record (PoH)", &metrics.record);
    print_row("Commit", &metrics.commit);
    println!("--------------------------------------------------------");
    print_row("Total Latency", &metrics.total);
    
    let p99_total = metrics.total.value_at_quantile(0.99);
    let budget_status = if p99_total <= VOTOR_DELTA_BLOCK_US { "PASS" } else { "VIOLATION" };
    
    println!("\n Votor Budget (Δ_block): {} us", VOTOR_DELTA_BLOCK_US);
    println!(" Matrix p99 Result     : {} us -> [{}]", p99_total, budget_status);
    println!("========================================================\n");
}

fn main() {
    agave_logger::setup();
    
    // Parse the first argument passed via CLI
    let args: Vec<String> = env::args().collect();
    let mode = if args.len() > 1 { args[1].as_str() } else { "baseline" };

    let profile = match mode {
        "cpu" => StressProfile::CpuStarvation,
        "io" => StressProfile::IoBlocking,
        "alloc" => StressProfile::AllocatorChurn,
        "baseline" | _ => StressProfile::Baseline,
    };

    execute_matrix_profile(profile);
}