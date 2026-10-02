//! All bot commands.

mod task;

pub use task::ComponentHandler;

use std::sync::Arc;

use crate::bot::{Data, Error};

/// The commands to register with Discord.
pub fn all() -> Vec<poise::Command<Arc<Data>, Error>> {
    vec![task::task()]
}
