//! MCP server and worker commands.

use crate::cli::{self, WorkerArgs};

pub fn serve() -> Result<(), String> {
    crate::mcp::serve()
}

pub fn worker(a: &WorkerArgs, deobf: bool) -> Result<(), String> {
    cli::run_big_stack(|| crate::mcp::worker(&a.path, deobf))
}
