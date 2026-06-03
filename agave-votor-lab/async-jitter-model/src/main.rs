use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    // Initialize tokio-console subscriber
    console_subscriber::init();
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "baseline".to_string());
    println!("Running async jitter model in mode: {}", mode);
    println!("Open a new terminal and run `tokio-console` to trace.");
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Producer Task (Simulating TPU Ingress)
    tokio::spawn(async move {
        loop {
            tx.send(()).await.unwrap();

            sleep(Duration::from_millis(1)).await;
        }
    });

    // Worker Task (Simulating Async Execution)
    loop {
        rx.recv().await.unwrap();
        match mode.as_str() {
            "cpu" => {
                // CPU Contention: Spikes scheduler delay, execution time stays flat
                let end = std::time::Instant::now() + Duration::from_millis(2);
                while std::time::Instant::now() < end {}
            }
            "io" => {
                // I/O Stall: Thread sleeps, massively spiking execution tail latency
                std::thread::sleep(Duration::from_millis(15));
            }
            "alloc" => {
                // Allocator Churn: Raises median execution time
                let _vec: Vec<u8> = vec![0; 10_000_000];
            }
            _ => {
                // Baseline: Smooth execution
                sleep(Duration::from_micros(150)).await;
            }
        }
    }
}