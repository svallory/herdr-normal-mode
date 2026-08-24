//! The herdr side of the plugin: everything here shells out to the `herdr`
//! CLI. Failures are swallowed on purpose — a popup that panics leaves the
//! terminal in raw mode, and a missed focus is recoverable while that is not.

use std::process::Command;

use crate::{Agent, Row, Workspace, parse_agent_list, parse_workspace_list};

/// herdr passes its own path to plugins; fall back to `$PATH` when running the
/// binary by hand.
pub fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}

/// `.output()` and not `.status()`: the CLI answers in JSON on stdout, which
/// would otherwise land on top of the popup's own drawing.
fn run(herdr: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(herdr).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn fetch_workspaces(herdr: &str) -> Vec<Workspace> {
    run(herdr, &["workspace", "list"])
        .map(|json| parse_workspace_list(&json))
        .unwrap_or_default()
}

pub fn fetch_agents(herdr: &str) -> Vec<Agent> {
    run(herdr, &["agent", "list"])
        .map(|json| parse_agent_list(&json))
        .unwrap_or_default()
}

/// Both lists are independent round trips to the herdr server, so run them at
/// the same time: the popup appears after the slower one rather than after the
/// sum of the two.
pub fn fetch_lists(herdr: &str) -> (Vec<Workspace>, Vec<Agent>) {
    std::thread::scope(|scope| {
        let workspaces = scope.spawn(|| fetch_workspaces(herdr));
        let agents = fetch_agents(herdr);
        // A panicked worker means no workspaces, not a dead popup.
        (workspaces.join().unwrap_or_default(), agents)
    })
}

/// Move herdr's focus onto the row under the cursor. `agent focus` takes a
/// pane id — a terminal id is rejected.
///
/// Reports whether herdr accepted it. The list is read once and never
/// refreshed, so a row can outlive its pane; the caller has to know a focus
/// failed or it would mark that row as reached and never try again.
pub fn focus_row(herdr: &str, row: &Row) -> bool {
    let args = match row {
        Row::Space { workspace_id, .. } => ["workspace", "focus", workspace_id.as_str()],
        Row::Agent { pane_id, .. } => ["agent", "focus", pane_id.as_str()],
    };
    Command::new(herdr)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}
