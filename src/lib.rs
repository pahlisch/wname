//! wname: a large, colored name band on top of a Hyprland window.
//!
//! The pure parts (colors, placement, IPC parsing, the control protocol) live
//! here so they can be tested without a compositor. `daemon` holds the GTK
//! side.

pub mod anim;
pub mod client;
pub mod color;
pub mod daemon;
pub mod hypr;
pub mod layout;
pub mod protocol;
pub mod target;
