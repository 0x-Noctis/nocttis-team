// Alias agar modul yang juga di-include langsung oleh test (`#[path]`) bisa merujuk `ai_team::...` yang sama.
extern crate self as ai_team;

pub mod agent;
pub mod api;
pub mod context;
pub mod domain;
pub mod model;
pub mod observability;
#[path = "model/openai/mod.rs"]
pub mod openai;
pub mod orchestrator;
pub mod recovery;
pub mod retention;
pub mod runner;
pub mod security;
pub mod store;
