//! Boot steps every board takes: find the partitions, open the store, get the node's identity
//! (stored, imported from the C++ firmware at first boot, or new: ADR-0006), name and PIN for
//! Bluetooth.

use core::cell::RefCell;

use embassy_embedded_hal::adapter::BlockingAsync;
use esp_bootloader_esp_idf::partitions::{PARTITION_TABLE_MAX_LEN, read_partition_table};
use esp_storage::FlashStorage;
use log::info;
use mc_air::crypto::LocalIdentity;
use mc_node::Prefs;
use mc_spiffs::{Spiffs, cpp};

use crate::flashstore::FlashStore;
use crate::partition::{ReadOnly, Region};

pub type Store<'a, 'd> = FlashStore<BlockingAsync<Region<'a, 'd>>>;

/// `(offset, length)` of the `mcdata` partition and of the C++ firmware's `spiffs`.
pub fn partitions(flash: &RefCell<FlashStorage<'_>>) -> (Option<(u32, u32)>, Option<(u32, u32)>) {
    let mut table = [0u8; PARTITION_TABLE_MAX_LEN];
    let mut f = flash.borrow_mut();
    let t = read_partition_table(&mut *f, &mut table).ok();
    let find = |label: &str| {
        t.as_ref()
            .and_then(|t| t.iter().find(|e| e.label_as_str() == label))
            .map(|e| (e.offset(), e.len()))
    };
    (find("mcdata"), find("spiffs"))
}

/// The store in `mcdata`. Panics without one: the node cannot keep anything.
pub fn open_store<'a, 'd>(
    flash: &'a RefCell<FlashStorage<'d>>,
    data: Option<(u32, u32)>,
) -> Store<'a, 'd> {
    let Some((off, len)) = data else {
        panic!("no mcdata partition")
    };
    let Some(store) = FlashStore::new(BlockingAsync::new(Region::new(flash, off, len)), len) else {
        panic!("mcdata too small")
    };
    store
}

/// The C++ firmware's file system, read only.
pub fn cpp_fs<'a, 'd>(
    flash: &'a RefCell<FlashStorage<'d>>,
    part: Option<(u32, u32)>,
) -> Option<Spiffs<ReadOnly<'a, 'd>>> {
    part.and_then(|(off, len)| Spiffs::new(ReadOnly::new(flash, off, len), len).ok())
}

/// The node's identity: the stored one; at first boot the C++ firmware's (with its settings,
/// contacts and channels), or a new one. A store that cannot be read is left alone (panic).
pub fn identity<'a, 'd: 'a>(
    store: &mut Store<'_, '_>,
    cpp: impl FnOnce() -> Option<Spiffs<ReadOnly<'a, 'd>>>,
    hw: &esp_hal::rng::Rng,
) -> LocalIdentity {
    match store.identity() {
        Ok(Some(prv)) => LocalIdentity::from_private(&prv),
        Err(()) => panic!("mcdata unreadable: the identity is left alone"),
        Ok(None) => {
            let imported = cpp().and_then(|mut fs| {
                match cpp::import(&mut fs, mc_node::node::UiPrefs::DEFAULT, store) {
                    Ok(s) => {
                        info!(
                            "import: C++ node, {} contacts, {} channels",
                            s.contacts, s.channels
                        );
                        store.identity().ok().flatten()
                    }
                    Err(e) => {
                        info!("import: nothing from the C++ firmware ({e:?})");
                        None
                    }
                }
            });
            if let Some(prv) = imported {
                return LocalIdentity::from_private(&prv);
            }
            let id = loop {
                let mut seed = [0u8; 32];
                for c in seed.chunks_exact_mut(4) {
                    c.copy_from_slice(&hw.random().to_le_bytes());
                }
                let id = LocalIdentity::from_seed(&seed);
                if !id.is_reserved() {
                    break id;
                }
            };
            mc_node::Storage::save_identity(store, id.private_key(), id.pub_key());
            id
        }
    }
}

/// The factory MAC address.
pub fn mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    for (d, s) in mac
        .iter_mut()
        .zip(esp_hal::efuse::base_mac_address().as_bytes())
    {
        *d = *s;
    }
    mac
}

/// The advertised Bluetooth name (`MeshCore-<name>`, `main.cpp:188-209`), fixed at boot.
pub fn ble_name(prefs: &Prefs, mac: [u8; 6]) -> heapless::Vec<u8, { mc_proto::ble::NAME_MAX }> {
    let mut name = [0u8; mc_proto::ble::NAME_MAX];
    let n = mc_proto::ble::device_name(mc_node::contacts::name_str(&prefs.name), mac, &mut name);
    let mut v = heapless::Vec::new();
    let _ = v.extend_from_slice(name.get(..n).unwrap_or(&[]));
    v
}

/// The PIN pairing asks for: the fixed one, or one drawn at boot (`main.cpp:188-202`).
pub fn pin_of(fixed: u32, hw: &esp_hal::rng::Rng) -> u32 {
    if fixed != 0 {
        fixed
    } else {
        100_000 + hw.random() % 900_000
    }
}
