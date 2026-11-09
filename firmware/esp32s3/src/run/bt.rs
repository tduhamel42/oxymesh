//! The Bluetooth host next to the main loop: `trouble-host` on the ESP32-S3's controller, the
//! companion service (`crate::ble`), bonds from flash. Without a controller the main loop runs
//! alone.

use core::future::Future;

use embassy_futures::join::join4;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::{
    Address, BondInformation, DefaultPacketPool, ExternalController, GapConfig, HostResources,
    IoCapabilities, PeripheralConfig, appearance,
};

use crate::ble::{self as ble, Server, Shared};
use crate::flashstore::BONDS_MAX;

/// Runs `app` (the board's main loop, and whatever it joins) next to the Bluetooth host.
pub async fn serve<A: Future>(
    connector: Option<BleConnector<'_>>,
    mac: [u8; 6],
    bonds: heapless::Vec<BondInformation, BONDS_MAX>,
    name: &[u8],
    sh: &Shared,
    app: A,
) {
    let Some(connector) = connector else {
        app.await;
        return;
    };
    // Static random address from the MAC (two top bits set): a phone that knew the C++
    // firmware sees a new device and pairs afresh.
    let mut addr = mac;
    addr[5] |= 0xC0;
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<_, DefaultPacketPool, 1, 2, 1, BONDS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(addr))
        .set_io_capabilities(IoCapabilities::DisplayOnly)
        .set_passkey(Some(sh.pin.get()))
        .build();
    for b in bonds {
        let _ = stack.add_bond_information(b);
    }
    let mut peripheral = stack.peripheral();
    let runner = stack.runner();
    let gap = core::str::from_utf8(name).map_or("MeshCore", ble::gap_name);
    match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: gap,
        appearance: &appearance::UNKNOWN,
    })) {
        Ok(server) => {
            join4(
                ble::run_host(runner),
                ble::serve(name, &mut peripheral, &server, sh),
                ble::control(&stack, sh),
                app,
            )
            .await;
        }
        Err(_) => {
            app.await;
        }
    }
}
