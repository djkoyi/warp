//! Command palette mode that lists the user's recent Claude Code conversations so one can be
//! resumed (`claude --resume <id>`) in a new tab.

mod data_source;
mod search_item;

pub use data_source::DataSource;
