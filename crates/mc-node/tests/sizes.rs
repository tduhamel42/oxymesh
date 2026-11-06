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
    println!("PacketBuf       {:>7}", size_of::<mc_air::PacketBuf>());
    assert!(
        size_of::<mc_node::Node>() < 128 * 1024,
        "the node must fit in 128 KB"
    );
}
