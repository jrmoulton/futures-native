use {std::time::Duration, time_native::interval};

fn main() {
    println!("Starting interval example...");

    let mut interval = interval(Duration::from_millis(500));

    for i in 1..=5 {
        let instant = blockon::block_on(interval.tick());
        println!("Tick {} at {:?}", i, instant);
    }

    println!("Interval example completed!");
}
