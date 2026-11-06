//! Memory sizes of the node structures (RAM budget of the ESP32-S3 without PSRAM, ADR-0003).

#[test]
fn print_sizes() {
    use core::mem::size_of;
    println!("Node            {:>7}", size_of::<mc_node::Node>());
    println!(
        "Contact         {:>7}",
        size_of::<mc_node::contacts::Contact>()
    );
    println!(
        "Contacts        {:>7}",
        size_of::<mc_node::contacts::Contacts>()
    );
    println!(
        "Channels        {:>7}",
        size_of::<mc_node::contacts::Channels>()
    );
    println!(
        "Dispatcher      {:>7}",
        size_of::<mc_node::dispatch::Dispatcher>()
    );
    println!("Event           {:>7}", size_of::<mc_node::Event>());
    println!("Device          {:>7}", size_of::<mc_node::Device>());
    println!(
        "History         {:>7}",
        size_of::<mc_node::history::History>()
    );
    println!("Heard           {:>7}", size_of::<mc_node::heard::Heard>());
    println!(
        "PacketLog       {:>7}",
        size_of::<mc_node::heard::PacketLog>()
    );
    println!("PacketBuf       {:>7}", size_of::<mc_air::PacketBuf>());
    assert!(
        size_of::<mc_node::Node>() < 128 * 1024,
        "the node must fit in 128 KB"
    );
    // Before U2 (offline queue in RAM): 147 KB. The screen's history must not cost more than
    // the queue that moved to flash.
    // (The simulator's `tap` feature adds event copies the device never has.)
    if !cfg!(feature = "tap") {
        assert!(
            size_of::<mc_node::Device>() < 150 * 1024,
            "the device must stay under 150 KB"
        );
    }
}
