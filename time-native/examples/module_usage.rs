use std::time::Duration;

fn main() {
    println!("Testing modular access...");

    // Test direct module access
    let sleep_future = time_native::sleep::sleep(Duration::from_millis(100));
    let _instant = blockon::block_on(sleep_future);
    println!("Sleep completed via sleep::sleep");

    // Test re-exported convenience functions
    let sleep_future2 = time_native::sleep(Duration::from_millis(100));
    let _instant2 = blockon::block_on(sleep_future2);
    println!("Sleep completed via re-exported sleep");

    // Test interval module access
    let mut interval = time_native::interval::interval(Duration::from_millis(200));
    let instant = blockon::block_on(interval.tick());
    println!("First interval tick at {:?}", instant);

    let instant2 = blockon::block_on(interval.tick());
    println!("Second interval tick at {:?}", instant2);

    println!("All module tests passed!");
}
