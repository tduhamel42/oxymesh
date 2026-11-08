//! The C++ firmware's SPIFFS (`mc-spiffs`): a flash image read and imported.
//!
//! Oracle: never panic and never loop forever on any image; what is read stays within the
//! files' sizes. The first 32 bytes of input choose the image size (in 4 KB blocks).
#![no_main]

use libfuzzer_sys::fuzz_target;
use mc_spiffs::{Spiffs, cpp};

fuzz_target!(|data: &[u8]| {
    let Ok(mut fs) = Spiffs::new(data, data.len() as u32) else {
        return;
    };
    let mut names: Vec<Vec<u8>> = Vec::new();
    let _ = fs.list(|n, _| names.push(n.to_vec()));
    for n in names.iter().take(8) {
        if let Ok(name) = core::str::from_utf8(n)
            && let Ok(f) = fs.open(name)
        {
            let mut buf = [0u8; 600];
            if let Ok(k) = fs.read_at(&f, 0, &mut buf) {
                assert!(k as u32 <= f.size);
            }
        }
    }
    let _ = cpp::summary(&mut fs);
});
