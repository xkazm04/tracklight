//! LightTrack MCP server — exposes read-mostly tools to Claude Code / agents.
//!
//! Planned tools (Phase 4):
//!   - query_traces(project?, since?, model?, limit)      → recent normalized events
//!   - get_cost_summary(project?, window)                 → cost/calls/tokens rollup
//!   - list_projects()                                    → projects + key prefixes
//!   - get_limit_status(project)                          → limit ratios + throttle flag
//!   - run_benchmark(benchmark_id)                        → enqueue a benchmark run
//!
//! Lets us dogfood: ask Claude Code "what did project X spend today?" straight from the terminal.

fn main() {
    println!("lighttrack-mcp v{} (scaffold)", env!("CARGO_PKG_VERSION"));
    for t in [
        "query_traces",
        "get_cost_summary",
        "list_projects",
        "get_limit_status",
        "run_benchmark",
    ] {
        println!("  tool: {t}");
    }
    println!("TODO(phase4): speak MCP over stdio, back tools with the Store");
}
