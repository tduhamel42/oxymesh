# Vendored crates

## trouble-host 0.7.0, with a fixed passkey

`trouble-host/` is trouble-host 0.7.0 from crates.io (embassy-rs/trouble, Apache-2.0 OR MIT,
© the Embassy project contributors), used through `[patch.crates-io]` in `../Cargo.toml`.

The one change, in `trouble-host-passkey.patch`, backports `set_passkey(Option<u32>)` from
trouble-host 0.8.0. It is the same code: a passkey set on the stack builder or the stack replaces
the random one drawn for each pairing in passkey-entry mode (DisplayOnly). MeshCore pairs with a
fixed PIN (`docs/spec/companion.md` §1.1), so we need it.

Why not 0.8.0 itself: it requires bt-hci 0.10, and esp-radio 1.0.0-beta.1 (the Bluetooth
controller) implements bt-hci 0.9. When esp-radio moves to bt-hci 0.10, drop this directory and
the patch section, and depend on trouble-host 0.8 or later: the calls stay the same.

No pull request upstream is needed: the feature is already there in 0.8 (ADR-0007).
