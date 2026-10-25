//! Cantor synthesis backends.

pub mod diffsinger;
pub mod onnx;
pub mod onnx_noise;
pub mod rmvpe;

pub use diffsinger::DiffSinger;
