//! Modular VRChat domain and media blocking system for LinuxVR (`lvr`).
//!
//! Provides granular blocking for:
//! - Video players (`urlList` pure video domains + yt-dlp stubbing)
//! - Image loading (`imageHostUrlList` pure image domains)
//! - String loading (`stringHostUrlList` pure string domains)
//!
//! Safety invariant:
//! Any domain that appears in two or more lists (e.g. `*.github.io`, `assets.vrchat.com`,
//! `*.v-market.work`, `*.poly.jp`, `ciel.topaz.chat`) is considered a shared asset
//! and is NEVER blocked by ANY toggle.

pub mod cache;
pub mod hosts;
pub mod parser;
pub mod types;

#[cfg(test)]
mod tests;

pub use cache::*;
pub use hosts::*;
#[allow(unused_imports)]
pub use parser::*;
pub use types::*;
