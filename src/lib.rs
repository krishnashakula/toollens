//! ToolLens: an MCP gateway that lets an agent discover, inspect and invoke tools
//! from a large catalog while only ever seeing a handful of them.

pub mod bench;
pub mod catalog;
pub mod config;
pub mod console;
pub mod discover;
pub mod fixtures;
pub mod gateway;
pub mod graph;
pub mod keyword;
pub mod lens;
pub mod meta;
pub mod policy;
pub mod semantic;
pub mod store;
pub mod text;
pub mod upstream;
pub mod validate;
