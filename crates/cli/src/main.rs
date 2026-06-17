//! `lt` — LightTrack operator CLI. A thin HTTP client over the API.
//!
//! Global options (also read from env):
//!   --base  LIGHTTRACK_URL  (default http://127.0.0.1:8787)
//!   --key   LIGHTTRACK_KEY  (admin key for management, or a project key for scoped reads)
//!
//! Examples:
//!   lt projects create --name billing-demo
//!   lt keys create --project <id> --name app-key
//!   lt limits set --project <id> --metric cost_usd --window day --threshold 5 --action alert
//!   lt limits status --project <id>
//!   lt costs --project <id>
//!   lt events --project <id> --limit 20

use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "lt", about = "LightTrack operator CLI")]
struct Cli {
    #[arg(long, env = "LIGHTTRACK_URL", default_value = "http://127.0.0.1:8787")]
    base: String,
    #[arg(long, env = "LIGHTTRACK_KEY")]
    key: Option<String>,
    /// Print raw JSON instead of the rendered table view (also implied when stdout is piped).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage projects.
    Projects {
        #[command(subcommand)]
        action: ProjectsCmd,
    },
    /// Manage API keys.
    Keys {
        #[command(subcommand)]
        action: KeysCmd,
    },
    /// Manage and inspect limit rules.
    Limits {
        #[command(subcommand)]
        action: LimitsCmd,
    },
    /// Cost/usage rollup.
    Costs {
        #[arg(long)]
        project: Option<String>,
    },
    /// Recent events.
    Events {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Profit margin: revenue − LLM cost by customer or product (default window: last 30 days).
    Margin {
        #[arg(long, default_value = "customer")]
        by: String,
        #[arg(long)]
        project: Option<String>,
        /// RFC3339 window start (default 30d ago).
        #[arg(long)]
        since: Option<String>,
        /// RFC3339 window end (default now).
        #[arg(long)]
        until: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectsCmd {
    Create {
        #[arg(long)]
        name: String,
    },
    List,
}

#[derive(Subcommand)]
enum KeysCmd {
    Create {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "default")]
        name: String,
    },
}

#[derive(Subcommand)]
enum LimitsCmd {
    Set {
        #[arg(long)]
        project: String,
        #[arg(long)]
        metric: String,
        #[arg(long)]
        window: String,
        #[arg(long)]
        threshold: f64,
        #[arg(long, default_value = "alert")]
        action: String,
    },
    List {
        #[arg(long)]
        project: String,
    },
    Status {
        #[arg(long)]
        project: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Projects { action } => match action {
            ProjectsCmd::Create { name } => {
                call(&cli, Method::POST, "/v1/projects", Some(json!({ "name": name })), "")
            }
            ProjectsCmd::List => call(&cli, Method::GET, "/v1/projects", None, "list_projects"),
        },
        Cmd::Keys { action } => match action {
            KeysCmd::Create { project, name } => call(
                &cli,
                Method::POST,
                &format!("/v1/projects/{project}/keys"),
                Some(json!({ "name": name })),
                "",
            ),
        },
        Cmd::Limits { action } => match action {
            LimitsCmd::Set {
                project,
                metric,
                window,
                threshold,
                action,
            } => call(
                &cli,
                Method::POST,
                &format!("/v1/projects/{project}/limits"),
                Some(json!({
                    "metric": metric, "window": window,
                    "threshold": threshold, "action": action
                })),
                "",
            ),
            LimitsCmd::List { project } => call(
                &cli,
                Method::GET,
                &format!("/v1/projects/{project}/limits"),
                None,
                "list_limits",
            ),
            LimitsCmd::Status { project } => call(
                &cli,
                Method::GET,
                &format!("/v1/limits/status?project={project}"),
                None,
                "get_limit_status",
            ),
        },
        Cmd::Costs { project } => call(
            &cli,
            Method::GET,
            &path_with_project("/v1/costs", project),
            None,
            "get_cost_summary",
        ),
        Cmd::Events { project, limit } => {
            let mut p = format!("/v1/events?limit={limit}");
            if let Some(proj) = project {
                p.push_str(&format!("&project={proj}"));
            }
            call(&cli, Method::GET, &p, None, "query_events")
        }
        Cmd::Margin { by, project, since, until } => {
            let mut p = format!("/v1/margin?by={by}");
            for (k, v) in [("project", project), ("since", since), ("until", until)] {
                if let Some(val) = v {
                    p.push_str(&format!("&{k}={val}"));
                }
            }
            call(&cli, Method::GET, &p, None, "get_margin")
        }
    }
}

fn path_with_project(base: &str, project: &Option<String>) -> String {
    match project {
        Some(p) => format!("{base}?project={p}"),
        None => base.to_string(),
    }
}

/// Issue one request and print the response, then exit non-zero on HTTP error. On a TTY (and unless
/// `--json`) a successful response is shown as a rendered Markdown table for `kind`; piped or `--json`
/// output stays raw JSON so scripts keep parsing it.
fn call(cli: &Cli, method: Method, path: &str, body: Option<Value>, kind: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let mut req = client.request(method, format!("{}{}", cli.base, path));
    if let Some(k) = &cli.key {
        req = req.bearer_auth(k);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req.send()?;
    let status = resp.status();
    let text = resp.text()?;
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => {
            let rendered = (!cli.json && status.is_success() && std::io::stdout().is_terminal())
                .then(|| lighttrack_render::render(kind, &v))
                .flatten();
            match rendered {
                Some(md) => println!("{md}"),
                None => println!("{}", serde_json::to_string_pretty(&v)?),
            }
        }
        Err(_) => println!("{text}"),
    }
    if !status.is_success() {
        eprintln!("HTTP {}", status.as_u16());
        std::process::exit(1);
    }
    Ok(())
}
