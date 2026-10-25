//! Cantor's desktop application. The GUI is one client of the engine: it
//! reads snapshots and sends Ops, and every action it offers has an Op and
//! a Python equivalent (see [`actions`]).

pub mod actions;
pub mod demo;
pub mod state;
pub mod theme;
pub mod view;
