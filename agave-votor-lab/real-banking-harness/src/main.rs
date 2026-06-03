use solana_measure::measure::Measure;
use hdrhistogram::Histogram;
use std::time::Duration;

const VOTOR_DELTA_BLOCK_MS: u64 = 400;


fn main() {
    println!("--- Agave BankingStage vs Votor Budget ---");
    println!("Target Votor Budget: {} ms", VOTOR_DELTA_BLOCK_MS);
    let mut histogram = Histogram::<u64>::new(3).unwrap();

    // Note: In a live Agave environment, this is where you boot the Bank,
    // GenesisConfig, and the BankingStage crossbeam channels.
    // For this harness, we simulate the black-box end-to-end timing loop.

    for i in 0..1000 {

        let mut measure = Measure::start("banking_stage_e2e");
        // Simulating the warm-up penalty on iter 0 (lazy loading, cache miss)
        // and standard fast execution on subsequent iters.
        if i == 0 {
            std::thread::sleep(Duration::from_micros(10_000));
        } else {
            std::thread::sleep(Duration::from_micros(150));
        }
        measure.stop();
        histogram.record(measure.as_us()).unwrap();
    }

    let p99_ms = histogram.value_at_quantile(0.99) as f64 / 1000.0;

    println!("Results:");
    println!("  P50 Latency: {} us", histogram.value_at_quantile(0.50));
    println!("  P99 Latency: {} ms", p99_ms);
    println!("  Headroom against Votor: {} ms", VOTOR_DELTA_BLOCK_MS as f64 - p99_ms);
}

