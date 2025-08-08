use {futures_util::StreamExt, std::time::Duration, time_native::interval};

fn main() {
    println!("Testing Interval as Stream...");

    // Create interval that ticks every 300ms
    let interval = interval(Duration::from_millis(300));

    // Use the interval as a stream and take the first 4 items
    let stream_future = async {
        let mut count = 0;
        let mut stream = interval.take(4);

        while let Some(instant) = stream.next().await {
            count += 1;
            println!("Stream tick {count} at {instant:?}");
        }

        println!("Stream completed after {count} ticks");
    };

    // Block on the stream processing
    blockon::block_on(stream_future);

    println!("Stream example completed!");
}
