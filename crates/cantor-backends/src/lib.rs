//! Cantor synthesis backends: DiffSinger (ONNX), UTAU (concatenative,
//! WORLD) and svs.io (external processes).

pub mod diffsinger;
pub mod flow;
pub mod onnx;
pub mod onnx_noise;
pub mod rmvpe;
pub mod svsio;
pub mod utau;
pub mod world;
mod zmtp;

pub use diffsinger::DiffSinger;
