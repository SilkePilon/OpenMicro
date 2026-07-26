#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

// Tests run against the no_std build too, so `std` has to be pulled in
// explicitly for them — the framing tests want `Vec` to collect frames.
#[cfg(test)]
extern crate std;

pub mod ble;
pub mod layout;
#[cfg(feature = "std")]
pub mod paths;
mod types;
pub mod wire;
pub use types::*;
