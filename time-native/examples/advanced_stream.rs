use {
    futures_util::StreamExt,
    std::time::{Duration, Instant},
    time_native::{interval, MissedTickBehavior},
};

fn main() {
    println!("Testing advanced Stream usage...");

    // Example 1: Using filter and map combinators
    let stream_future = async {
        let interval = interval(Duration::from_millis(100));
        let start_time = Instant::now();
        
        let mut processed_stream = interval
            .take(10)
            .enumerate()
            .filter(|(i, _)| futures_util::future::ready(*i % 2 == 0)) // Only even indices
            .map(|(i, instant)| {
                let elapsed = instant.duration_since(start_time);
                (i, elapsed)
            });
        
        println!("Processing filtered stream...");
        while let Some((index, elapsed)) = processed_stream.next().await {
            println!("  Even tick {} at +{:?}", index, elapsed);
        }
    };
    
    blockon::block_on(stream_future);
    
    // Example 2: Using missed tick behavior
    let behavior_future = async {
        println!("\nTesting missed tick behavior...");
        let mut interval = interval(Duration::from_millis(50));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        
        let mut count = 0;
        let mut stream = interval.take(3);
        let start = Instant::now();
        
        while let Some(instant) = stream.next().await {
            count += 1;
            let elapsed = instant.duration_since(start);
            println!("  Tick {} at +{:?}", count, elapsed);
            
            // Simulate some work that might cause missed ticks
            if count == 2 {
                std::thread::sleep(Duration::from_millis(120)); // Cause missed ticks
            }
        }
    };
    
    blockon::block_on(behavior_future);
    
    println!("Advanced stream example completed!");
}