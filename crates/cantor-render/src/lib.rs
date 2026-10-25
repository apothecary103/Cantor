//! Cantor's render pipeline: phrases, base pitch, the content-addressed
//! cache, and mixing. Backends (cantor-backends) do the neural work.

#![forbid(unsafe_code)]

pub mod backend;
pub mod cache;
pub mod key;
pub mod mix;
pub mod phrase;
pub mod pitch;
pub mod render;
pub mod rng;
pub mod wav;
