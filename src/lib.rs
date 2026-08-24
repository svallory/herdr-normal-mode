use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use serde::Deserialize;

pub mod herdr;

/// A run of text on a row, with the SGR parameter herdr's own prefix bar would
/// draw it with: `7` reverse video for the mode chip, `1` bright for a key, `2`
/// dim for what the key does. Spacing lives inside the dim spans, where it is
/// invisible either way.
struct Span {
    sgr: &'static str,
    text: &'static str,
}

/// The first row: that the mode is on, as a chip like herdr's own `PREFIX`.
const MODE_ROW: &[Span] = &[Span { sgr: "7", text: " NORMAL " }];

/// The second row: the keys and what they do. Which row the cursor is on is
/// deliberately absent — the sidebar highlights it already, because every move
/// really moves herdr's focus.
const KEYS_ROW: &[Span] = &[
    Span { sgr: "1", text: "j/k" },
    Span { sgr: "2", text: " row  " },
    Span { sgr: "1", text: "h/l" },
    Span { sgr: "2", text: " tab  " },
    Span { sgr: "1", text: "i" },
    Span { sgr: "2", text: " exit" },
];

/// Each span in its own SGR, reset after every one of them: an escape left open
/// would tint whatever herdr draws next.
fn render_row(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|span| format!("\x1b[{}m{}\x1b[0m", span.sgr, span.text))
        .collect()
}

pub fn mode_row() -> String {
    render_row(MODE_ROW)
}

pub fn keys_row() -> String {
    render_row(KEYS_ROW)
}

/// A workspace as the sidebar shows it, in the order `workspace list` returns.
/// `active_tab_id` is where `h` and `l` start walking that workspace's tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub workspace_id: String,
    pub active_tab_id: Option<String>,
}

/// An agent pane as the sidebar shows it, in the order `agent list` returns.
/// The owning workspace is deliberately absent: the rows are one flat list in
/// the order the CLI gave them, so nothing groups or filters by workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane_id: String,
    /// Which workspace's tabs `h` and `l` walk while the cursor is on this
    /// agent. Optional so an entry that does not say drops its tab stepping
    /// rather than dropping itself out of the list.
    pub workspace_id: Option<String>,
}

/// A tab as `tab list` returns it. The label is absent on purpose: herdr's own
/// tab bar shows it, and that is where the user is looking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
}

/// One selectable sidebar line. Spaces come first, then agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Space { workspace_id: String },
    Agent { pane_id: String, workspace_id: Option<String> },
}

impl Row {
    /// The workspace whose tabs `h` and `l` walk while the cursor is here.
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Row::Space { workspace_id } => Some(workspace_id),
            Row::Agent { workspace_id, .. } => workspace_id.as_deref(),
        }
    }
}

/// What a keystroke means. Deliberately free of side effects: the caller owns
/// the herdr calls so the key handling stays testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Move(isize),
    Tab(isize),
    Quit,
    Ignore,
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

/// The slice of herdr's `PluginInvocationContext` we care about, handed to the
/// pane as `HERDR_PLUGIN_CONTEXT_JSON`. For a popup it records where the focus
/// was immediately before we opened.
#[derive(Debug, Default, Deserialize)]
pub struct InvocationContext {
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

pub fn parse_invocation_context(json: &str) -> InvocationContext {
    serde_json::from_str(json).unwrap_or_default()
}

#[derive(Deserialize)]
struct WorkspaceListResult {
    #[serde(default)]
    workspaces: Vec<RawWorkspace>,
}

/// Ids are optional so one malformed entry drops itself rather than the whole
/// list.
#[derive(Deserialize)]
struct RawWorkspace {
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    active_tab_id: Option<String>,
}

#[derive(Deserialize)]
struct TabListResult {
    #[serde(default)]
    tabs: Vec<RawTab>,
}

/// Both ids are needed: one to focus the tab, the other to know whose tab it is.
/// An entry missing either drops itself rather than the whole list.
#[derive(Deserialize)]
struct RawTab {
    #[serde(default)]
    tab_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct AgentListResult {
    #[serde(default)]
    agents: Vec<RawAgent>,
}

/// The pane id is the only field we read. It is also the only one every agent
/// in a real `herdr agent list` carries — the rest of the shape varies per
/// agent, which is exactly why nothing else is relied on.
#[derive(Deserialize)]
struct RawAgent {
    #[serde(default)]
    pane_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
}

/// A present-but-blank field carries no more information than a missing one,
/// so both collapse to `None`.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Work out where the focus was just before the popup took it.
///
/// `HERDR_PLUGIN_CONTEXT_JSON` is the only thing that can name the pane: the
/// one pane id in a plugin pane's environment is `HERDR_PANE_ID`, which is the
/// plugin's own and would point the cursor at ourselves. The workspace does
/// have an independent source (`HERDR_WORKSPACE_ID`), so it falls back on its
/// own and a context with only a pane still leaves that usable.
pub fn resolve_active_ids(
    context_json: Option<&str>,
    env_workspace_id: Option<&str>,
) -> (Option<String>, Option<String>) {
    let context = context_json.map(parse_invocation_context).unwrap_or_default();
    let pane = non_blank(context.focused_pane_id.as_deref()).map(str::to_string);
    let workspace = non_blank(context.workspace_id.as_deref())
        .or_else(|| non_blank(env_workspace_id))
        .map(str::to_string);
    (pane, workspace)
}

/// Malformed or unexpected JSON yields an empty list rather than a panic: this
/// runs inside a popup where a crash leaves the terminal in raw mode.
pub fn parse_workspace_list(json: &str) -> Vec<Workspace> {
    let Ok(envelope) = serde_json::from_str::<Envelope<WorkspaceListResult>>(json) else {
        return Vec::new();
    };
    envelope
        .result
        .workspaces
        .into_iter()
        .filter_map(|raw| {
            Some(Workspace {
                workspace_id: raw.workspace_id?,
                active_tab_id: non_blank(raw.active_tab_id.as_deref()).map(str::to_string),
            })
        })
        .collect()
}

pub fn parse_agent_list(json: &str) -> Vec<Agent> {
    let Ok(envelope) = serde_json::from_str::<Envelope<AgentListResult>>(json) else {
        return Vec::new();
    };
    envelope
        .result
        .agents
        .into_iter()
        .filter_map(|raw| {
            Some(Agent {
                pane_id: raw.pane_id?,
                workspace_id: non_blank(raw.workspace_id.as_deref()).map(str::to_string),
            })
        })
        .collect()
}

/// Every tab in the session, in the order `tab list` returned them — which is
/// the order herdr's own tab bar shows them in, grouped by workspace.
pub fn parse_tab_list(json: &str) -> Vec<Tab> {
    let Ok(envelope) = serde_json::from_str::<Envelope<TabListResult>>(json) else {
        return Vec::new();
    };
    envelope
        .result
        .tabs
        .into_iter()
        .filter_map(|raw| {
            Some(Tab {
                tab_id: raw.tab_id?,
                workspace_id: raw.workspace_id?,
            })
        })
        .collect()
}

/// One continuous list matching the sidebar: the spaces section, then the
/// agents section. Both keep the order the CLI returned them in.
pub fn build_rows(workspaces: &[Workspace], agents: &[Agent], own_pane_id: Option<&str>) -> Vec<Row> {
    let spaces = workspaces.iter().map(|workspace| Row::Space {
        workspace_id: workspace.workspace_id.clone(),
    });
    let agents = agents
        .iter()
        .filter(|agent| Some(agent.pane_id.as_str()) != own_pane_id)
        .map(|agent| Row::Agent {
            pane_id: agent.pane_id.clone(),
            workspace_id: agent.workspace_id.clone(),
        });
    spaces.chain(agents).collect()
}

/// The popup owns the focus by the time we run, so the `focused` flags in the
/// CLI output are useless. The invocation context tells us where the user was.
///
/// `None` means no row matched. That is not the same as row 0: the caller
/// still starts the cursor at the top, but it must not assume herdr's focus is
/// already there, or the first thing the user does would send no focus at all.
pub fn initial_cursor(
    rows: &[Row],
    active_pane_id: Option<&str>,
    active_workspace_id: Option<&str>,
) -> Option<usize> {
    let agent = active_pane_id.and_then(|active| {
        rows.iter()
            .position(|row| matches!(row, Row::Agent { pane_id, .. } if pane_id == active))
    });
    let space = || {
        active_workspace_id.and_then(|active| {
            rows.iter()
                .position(|row| matches!(row, Row::Space { workspace_id, .. } if workspace_id == active))
        })
    };
    agent.or_else(space)
}

/// The tabs of one workspace, and which of them we last moved to.
struct Ring {
    workspace_id: String,
    tab_ids: Vec<String>,
    cursor: usize,
}

/// Every workspace's tabs, each with a cursor of its own.
///
/// Read once when the popup opens and stepped locally from then on, for the
/// same reason the rows are: after the first `l` the only thing a re-read would
/// tell us is where we ourselves moved the focus to.
pub struct Tabs {
    rings: Vec<Ring>,
}

impl Tabs {
    /// One ring per workspace, in workspace order, each starting on the tab
    /// that workspace was already showing.
    pub fn new(workspaces: &[Workspace], tabs: &[Tab]) -> Self {
        let rings = workspaces
            .iter()
            .map(|workspace| {
                let tab_ids: Vec<String> = tabs
                    .iter()
                    .filter(|tab| tab.workspace_id == workspace.workspace_id)
                    .map(|tab| tab.tab_id.clone())
                    .collect();
                let cursor = workspace
                    .active_tab_id
                    .as_deref()
                    .and_then(|active| tab_ids.iter().position(|tab_id| tab_id == active))
                    .unwrap_or(0);
                Ring {
                    workspace_id: workspace.workspace_id.clone(),
                    tab_ids,
                    cursor,
                }
            })
            .collect();
        Self { rings }
    }

    /// Step `delta` tabs in `workspace_id` and name the tab to focus, wrapping
    /// at both ends the way the rows do.
    ///
    /// `None` when there is nowhere to step: a workspace we were told nothing
    /// about, or one with a single tab. Naming its one tab would send herdr a
    /// focus that changes nothing.
    pub fn step(&mut self, workspace_id: &str, delta: isize) -> Option<&str> {
        let ring = self
            .rings
            .iter_mut()
            .find(|ring| ring.workspace_id == workspace_id)?;
        if ring.tab_ids.len() < 2 {
            return None;
        }
        ring.cursor = move_cursor(ring.cursor, ring.tab_ids.len(), delta);
        Some(&ring.tab_ids[ring.cursor])
    }
}

pub fn move_cursor(cursor: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    let current = (cursor as isize).min(len - 1);
    (current + delta).rem_euclid(len) as usize
}

pub fn handle_key(event: &KeyEvent) -> Action {
    // kitty keyboard protocol and Windows also deliver Release and Repeat;
    // acting on those turns one keystroke into two moves.
    if event.kind != KeyEventKind::Press {
        return Action::Ignore;
    }
    let bare = event.modifiers.is_empty();
    match event.code {
        KeyCode::Char('c') if event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            Action::Quit
        }
        KeyCode::Char('j') if bare => Action::Move(1),
        KeyCode::Char('k') if bare => Action::Move(-1),
        KeyCode::Char('l') if bare => Action::Tab(1),
        KeyCode::Char('h') if bare => Action::Tab(-1),
        KeyCode::Char('i') | KeyCode::Enter if bare => Action::Quit,
        _ => Action::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyEventState, KeyModifiers};

    /// Trimmed from a real `herdr workspace list` response.
    const WORKSPACE_LIST_JSON: &str = r#"{
        "id": "cli:workspace:list",
        "result": {
            "type": "workspace_list",
            "workspaces": [
                {"active_tab_id":"w7:t4","agent_status":"unknown","focused":false,"label":"tools","number":1,"pane_count":1,"tab_count":1,"workspace_id":"w7"},
                {"active_tab_id":"w9:t2J","agent_status":"working","focused":true,"label":"work","number":2,"pane_count":5,"tab_count":2,"workspace_id":"w9"},
                {"active_tab_id":"wB:t2","agent_status":"working","focused":false,"label":"personal","number":3,"pane_count":1,"tab_count":1,"workspace_id":"wB"}
            ]
        }
    }"#;

    /// Trimmed from a real `herdr agent list` response. Every field but
    /// `pane_id` is left in to keep the fixture honest about how much of this
    /// the parser walks past: the second agent is the only one carrying `name`,
    /// and the third has none at all.
    const AGENT_LIST_JSON: &str = r#"{
        "id": "cli:agent:list",
        "result": {
            "type": "agent_list",
            "agents": [
                {"agent":"claude","agent_status":"idle","cwd":"/home/user/src/notes","focused":false,"pane_id":"w9:p9P","tab_id":"w9:t2J","terminal_id":"term_659aedd8440794","terminal_title":"✳ 各種タスクのスケジュール調整","terminal_title_stripped":"各種タスクのスケジュール調整","workspace_id":"w9"},
                {"agent":"claude","agent_status":"working","cwd":"/home/user/src/webapp","focused":false,"interactive_ready":true,"name":"login-fix","pane_id":"w9:pEV","tab_id":"w9:t4T","terminal_id":"term_659c050adb4d21a","terminal_title":"✳ fix-login-redirect","terminal_title_stripped":"fix-login-redirect","workspace_id":"w9"},
                {"agent":"claude","agent_status":"working","cwd":"/home/user/src/notes","focused":false,"pane_id":"wB:p2","tab_id":"wB:t2","terminal_id":"term_659c1c88dd1321d","terminal_title":"◑ vim-normal","terminal_title_stripped":"vim-normal","workspace_id":"wB"}
            ]
        }
    }"#;

    /// Trimmed from a real `herdr tab list` response. The tab ids match the
    /// `active_tab_id` of the workspace fixture, so the two describe one
    /// session: w7 and wB have a single tab each, w9 has three.
    const TAB_LIST_JSON: &str = r#"{
        "id": "cli:tab:list",
        "result": {
            "type": "tab_list",
            "tabs": [
                {"agent_status":"unknown","focused":false,"label":"devtools/master","number":4,"pane_count":1,"tab_id":"w7:t4","workspace_id":"w7"},
                {"agent_status":"idle","focused":false,"label":"torudo/main","number":82,"pane_count":3,"tab_id":"w9:t2J","workspace_id":"w9"},
                {"agent_status":"working","focused":false,"label":"jfa-apps/remove-list","number":156,"pane_count":1,"tab_id":"w9:t4W","workspace_id":"w9"},
                {"agent_status":"working","focused":false,"label":"ga-pms/bv/table","number":158,"pane_count":1,"tab_id":"w9:t4Y","workspace_id":"w9"},
                {"agent_status":"working","focused":true,"label":"herdr-normal-mode/main","number":6,"pane_count":1,"tab_id":"wB:t2","workspace_id":"wB"}
            ]
        }
    }"#;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn space(id: &str) -> Row {
        Row::Space {
            workspace_id: id.into(),
        }
    }

    fn agent_row(id: &str, workspace_id: &str) -> Row {
        Row::Agent {
            pane_id: id.into(),
            workspace_id: Some(workspace_id.into()),
        }
    }

    /// The three spaces followed by the three agents, as the fixtures describe
    /// them. `rows_run_spaces_first_then_agents` pins down what this contains.
    fn full_rows() -> Vec<Row> {
        build_rows(
            &parse_workspace_list(WORKSPACE_LIST_JSON),
            &parse_agent_list(AGENT_LIST_JSON),
            None,
        )
    }

    /// The same list with the agents section empty.
    fn space_rows() -> Vec<Row> {
        build_rows(&parse_workspace_list(WORKSPACE_LIST_JSON), &[], None)
    }

    #[test]
    fn parses_workspaces_in_cli_order() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST_JSON);

        assert_eq!(
            workspaces,
            vec![
                Workspace { workspace_id: "w7".into(), active_tab_id: Some("w7:t4".into()) },
                Workspace { workspace_id: "w9".into(), active_tab_id: Some("w9:t2J".into()) },
                Workspace { workspace_id: "wB".into(), active_tab_id: Some("wB:t2".into()) },
            ]
        );
    }

    #[test]
    fn parses_agents_in_cli_order() {
        let agents = parse_agent_list(AGENT_LIST_JSON);

        assert_eq!(
            agents,
            vec![
                Agent { pane_id: "w9:p9P".into(), workspace_id: Some("w9".into()) },
                Agent { pane_id: "w9:pEV".into(), workspace_id: Some("w9".into()) },
                Agent { pane_id: "wB:p2".into(), workspace_id: Some("wB".into()) },
            ]
        );
    }

    #[test]
    fn broken_json_yields_empty_lists_instead_of_panicking() {
        for json in ["", "not json", "{}", r#"{"result":{}}"#, r#"{"result":{"workspaces":"nope"}}"#] {
            assert!(parse_workspace_list(json).is_empty(), "workspaces from {json:?}");
            assert!(parse_agent_list(json).is_empty(), "agents from {json:?}");
        }
    }

    #[test]
    fn agent_without_pane_id_is_dropped() {
        let json = r#"{"result":{"agents":[
            {"workspace_id":"w1","name":"ghost"},
            {"pane_id":"w1:p1","workspace_id":"w1","name":"real"}
        ]}}"#;

        let agents = parse_agent_list(json);

        assert_eq!(
            agents,
            vec![Agent { pane_id: "w1:p1".into(), workspace_id: Some("w1".into()) }]
        );
    }

    #[test]
    fn workspace_without_an_id_is_dropped() {
        let json = r#"{"result":{"workspaces":[
            {"label":"ghost"},
            {"workspace_id":"w1","label":"real"}
        ]}}"#;

        let workspaces = parse_workspace_list(json);

        assert_eq!(
            workspaces,
            vec![Workspace { workspace_id: "w1".into(), active_tab_id: None }]
        );
    }

    #[test]
    fn invocation_context_gives_the_focus_from_just_before_the_popup() {
        let json = r#"{
            "workspace_id": "w9",
            "workspace_label": "work",
            "tab_id": "w9:t2J",
            "focused_pane_id": "w9:p9P",
            "focused_pane_agent": "claude",
            "invocation_source": "plugin-pane"
        }"#;

        let context = parse_invocation_context(json);

        assert_eq!(context.focused_pane_id.as_deref(), Some("w9:p9P"));
        assert_eq!(context.workspace_id.as_deref(), Some("w9"));
    }

    #[test]
    fn a_missing_or_broken_invocation_context_is_simply_empty() {
        for json in ["", "not json", "{}"] {
            let context = parse_invocation_context(json);
            assert!(context.focused_pane_id.is_none(), "{json:?}");
            assert!(context.workspace_id.is_none(), "{json:?}");
        }
    }

    #[test]
    fn active_ids_prefer_the_invocation_context_over_the_environment() {
        let context = r#"{"workspace_id":"w9","focused_pane_id":"w9:p9P"}"#;

        let (pane, workspace) = resolve_active_ids(Some(context), Some("wB"));

        assert_eq!(pane.as_deref(), Some("w9:p9P"));
        assert_eq!(workspace.as_deref(), Some("w9"));
    }

    #[test]
    fn the_workspace_falls_back_to_the_environment_on_its_own() {
        // A context carrying only the pane still lets the environment name the
        // workspace; the two resolve independently.
        let context = r#"{"focused_pane_id":"w9:p9P"}"#;

        let (pane, workspace) = resolve_active_ids(Some(context), Some("wB"));

        assert_eq!(pane.as_deref(), Some("w9:p9P"));
        assert_eq!(workspace.as_deref(), Some("wB"));
    }

    #[test]
    fn the_workspace_falls_back_when_there_is_no_context_at_all() {
        for context in [None, Some(""), Some("not json")] {
            let (pane, workspace) = resolve_active_ids(context, Some("wB"));

            // Nothing outside the context can name the focused pane: the only
            // pane id in a plugin pane's environment is the plugin's own.
            assert!(pane.is_none(), "{context:?}");
            assert_eq!(workspace.as_deref(), Some("wB"), "{context:?}");
        }
    }

    #[test]
    fn blank_active_ids_count_as_absent() {
        let context = r#"{"workspace_id":"","focused_pane_id":"   "}"#;

        let (pane, workspace) = resolve_active_ids(Some(context), Some("  "));

        assert!(pane.is_none());
        assert!(workspace.is_none());
    }

    #[test]
    fn active_ids_are_none_when_nothing_says_where_we_were() {
        let (pane, workspace) = resolve_active_ids(None, None);

        assert!(pane.is_none());
        assert!(workspace.is_none());
    }

    #[test]
    fn rows_run_spaces_first_then_agents() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST_JSON);
        let agents = parse_agent_list(AGENT_LIST_JSON);

        let rows = build_rows(&workspaces, &agents, None);

        assert_eq!(
            rows,
            vec![
                space("w7"),
                space("w9"),
                space("wB"),
                agent_row("w9:p9P", "w9"),
                agent_row("w9:pEV", "w9"),
                agent_row("wB:p2", "wB"),
            ]
        );
    }

    #[test]
    fn our_own_pane_is_left_out_of_the_rows() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST_JSON);
        let agents = parse_agent_list(AGENT_LIST_JSON);

        let rows = build_rows(&workspaces, &agents, Some("w9:pEV"));

        assert_eq!(rows.len(), 5);
        assert!(!rows.contains(&agent_row("w9:pEV", "w9")));
    }

    #[test]
    fn rows_hold_up_with_no_agents_at_all() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST_JSON);

        let rows = build_rows(&workspaces, &[], None);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], space("w7"));
    }

    #[test]
    fn cursor_starts_on_the_agent_that_had_focus() {
        let rows = full_rows();

        assert_eq!(initial_cursor(&rows, Some("wB:p2"), Some("wB")), Some(5));
    }

    #[test]
    fn cursor_falls_back_to_the_workspace_when_the_pane_is_not_an_agent() {
        let rows = full_rows();

        // A plain shell pane in w9 is not in the agents section, so the cursor
        // lands on the w9 space row instead of defaulting to the top.
        assert_eq!(initial_cursor(&rows, Some("w9:p1"), Some("w9")), Some(1));
    }

    #[test]
    fn no_match_leaves_the_starting_row_undecided() {
        // The caller starts at the top either way, but it has to be able to
        // tell "row 0 is where the focus already is" from "we have no idea":
        // in the second case the first move has to actually send a focus.
        let rows = space_rows();

        assert_eq!(initial_cursor(&rows, Some("zz:p9"), Some("zz")), None);
        assert_eq!(initial_cursor(&rows, None, None), None);
        assert_eq!(initial_cursor(&[], Some("w9:p9P"), Some("w9")), None);
    }

    #[test]
    fn moving_the_cursor_wraps_at_both_ends() {
        assert_eq!(move_cursor(0, 3, 1), 1);
        assert_eq!(move_cursor(2, 3, 1), 0);
        assert_eq!(move_cursor(0, 3, -1), 2);
        assert_eq!(move_cursor(2, 3, -1), 1);
    }

    #[test]
    fn moving_the_cursor_survives_an_empty_or_degenerate_list() {
        assert_eq!(move_cursor(0, 0, 1), 0);
        assert_eq!(move_cursor(0, 0, -1), 0);
        assert_eq!(move_cursor(0, 1, 1), 0);
        assert_eq!(move_cursor(0, 1, -1), 0);
        // A cursor past the end is clamped to the last row first, so moving
        // down from it wraps to the top.
        assert_eq!(move_cursor(9, 3, 1), 0);
        assert_eq!(move_cursor(9, 3, -1), 1);
    }

    #[test]
    fn j_and_k_move_only_without_modifiers() {
        assert_eq!(handle_key(&press(KeyCode::Char('j'), KeyModifiers::NONE)), Action::Move(1));
        assert_eq!(handle_key(&press(KeyCode::Char('k'), KeyModifiers::NONE)), Action::Move(-1));
        assert_eq!(handle_key(&press(KeyCode::Char('j'), KeyModifiers::CONTROL)), Action::Ignore);
        assert_eq!(handle_key(&press(KeyCode::Char('k'), KeyModifiers::ALT)), Action::Ignore);
        assert_eq!(handle_key(&press(KeyCode::Char('J'), KeyModifiers::SHIFT)), Action::Ignore);
    }

    #[test]
    fn i_enter_and_ctrl_c_quit() {
        assert_eq!(handle_key(&press(KeyCode::Char('i'), KeyModifiers::NONE)), Action::Quit);
        assert_eq!(handle_key(&press(KeyCode::Enter, KeyModifiers::NONE)), Action::Quit);
        assert_eq!(handle_key(&press(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Quit);
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        for code in [KeyCode::Char('x'), KeyCode::Char('g'), KeyCode::Down, KeyCode::Tab] {
            assert_eq!(handle_key(&press(code, KeyModifiers::NONE)), Action::Ignore);
        }
    }

    #[test]
    fn only_press_events_count() {
        // kitty keyboard protocol and Windows both deliver Release/Repeat
        // events; acting on them makes one keystroke move two rows.
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            let event = KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                kind,
                state: KeyEventState::NONE,
            };
            assert_eq!(handle_key(&event), Action::Ignore);
        }
    }

    /// The popup says two things and no more: that the mode is on, and how to
    /// work it. Which row the cursor is on is not among them — the sidebar
    /// highlights that itself, since every move really does shift the focus.
    #[test]
    fn the_rows_name_the_mode_and_the_keys() {
        let mode = mode_row();
        let keys = keys_row();

        assert!(mode.contains("NORMAL"), "{mode:?}");
        for part in ["j/k", "row", "h/l", "tab", "i", "exit"] {
            assert!(keys.contains(part), "{part} missing from {keys:?}");
        }
    }

    /// The rows go to the pane as they stand, with no fitting done at runtime,
    /// so they have to fit the popup the manifest asks for. Measured: herdr
    /// hands a popup its outer width less three columns, so the 28 in the
    /// manifest leaves 25. A longer row would wrap, and the wrap would push the
    /// other row out of the two the popup has.
    ///
    /// Being ASCII is what lets the row be measured in bytes: a wide character
    /// would take two columns while counting as more bytes still.
    #[test]
    fn the_rows_fit_the_popup_the_manifest_asks_for() {
        for spans in [MODE_ROW, KEYS_ROW] {
            let text: String = spans.iter().map(|span| span.text).collect();

            assert!(text.is_ascii(), "{text:?}");
            assert!(text.len() <= 25, "{text:?} is {} columns", text.len());
        }
    }

    #[test]
    fn parses_tabs_in_cli_order_with_their_workspace() {
        let tabs = parse_tab_list(TAB_LIST_JSON);

        assert_eq!(
            tabs,
            vec![
                Tab { tab_id: "w7:t4".into(), workspace_id: "w7".into() },
                Tab { tab_id: "w9:t2J".into(), workspace_id: "w9".into() },
                Tab { tab_id: "w9:t4W".into(), workspace_id: "w9".into() },
                Tab { tab_id: "w9:t4Y".into(), workspace_id: "w9".into() },
                Tab { tab_id: "wB:t2".into(), workspace_id: "wB".into() },
            ]
        );
    }

    #[test]
    fn a_tab_missing_either_id_is_dropped() {
        let json = r#"{"result":{"tabs":[
            {"workspace_id":"w1"},
            {"tab_id":"w1:t9"},
            {"tab_id":"w1:t1","workspace_id":"w1"}
        ]}}"#;

        let tabs = parse_tab_list(json);

        assert_eq!(
            tabs,
            vec![Tab { tab_id: "w1:t1".into(), workspace_id: "w1".into() }]
        );
    }

    #[test]
    fn broken_tab_json_yields_an_empty_list() {
        for json in ["", "not json", "{}", r#"{"result":{"tabs":"nope"}}"#] {
            assert!(parse_tab_list(json).is_empty(), "tabs from {json:?}");
        }
    }

    /// A row has to say which workspace's tabs `h` and `l` walk. A space row is
    /// its own answer; an agent row carries the workspace it lives in.
    #[test]
    fn a_row_names_the_workspace_whose_tabs_it_walks() {
        let rows = full_rows();

        assert_eq!(rows[1].workspace_id(), Some("w9"));
        assert_eq!(rows[5].workspace_id(), Some("wB"));
    }

    /// Stepping starts from the tab the workspace was already on, not from its
    /// first tab: pressing `l` on a space should land on the tab after the one
    /// the user was looking at.
    #[test]
    fn stepping_tabs_starts_from_the_tab_the_workspace_was_on() {
        let mut tabs = Tabs::new(
            &parse_workspace_list(WORKSPACE_LIST_JSON),
            &parse_tab_list(TAB_LIST_JSON),
        );

        // w9 was on w9:t2J, the first of its three.
        assert_eq!(tabs.step("w9", 1), Some("w9:t4W"));
        assert_eq!(tabs.step("w9", 1), Some("w9:t4Y"));
    }

    #[test]
    fn stepping_tabs_wraps_at_both_ends() {
        let mut tabs = Tabs::new(
            &parse_workspace_list(WORKSPACE_LIST_JSON),
            &parse_tab_list(TAB_LIST_JSON),
        );

        // Back off the front of w9's three tabs and round to the last.
        assert_eq!(tabs.step("w9", -1), Some("w9:t4Y"));
        assert_eq!(tabs.step("w9", 1), Some("w9:t2J"));
    }

    /// One tab is nowhere to go, and an unknown workspace is nothing to go in.
    /// Both have to say so rather than name a tab, or the popup would send herdr
    /// a focus that changes nothing.
    #[test]
    fn a_workspace_with_nothing_to_step_through_says_so() {
        let mut tabs = Tabs::new(
            &parse_workspace_list(WORKSPACE_LIST_JSON),
            &parse_tab_list(TAB_LIST_JSON),
        );

        assert_eq!(tabs.step("w7", 1), None);
        assert_eq!(tabs.step("zz", 1), None);
    }

    /// A workspace herdr listed no tabs for is not a panic.
    #[test]
    fn tabs_hold_up_when_a_workspace_has_no_tabs_listed() {
        let mut tabs = Tabs::new(&parse_workspace_list(WORKSPACE_LIST_JSON), &[]);

        assert_eq!(tabs.step("w9", 1), None);
    }

    #[test]
    fn h_and_l_step_tabs_only_without_modifiers() {
        assert_eq!(handle_key(&press(KeyCode::Char('l'), KeyModifiers::NONE)), Action::Tab(1));
        assert_eq!(handle_key(&press(KeyCode::Char('h'), KeyModifiers::NONE)), Action::Tab(-1));
        assert_eq!(
            handle_key(&press(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Action::Ignore
        );
    }

    /// An SGR left open would carry into whatever herdr draws after the popup.
    #[test]
    fn every_span_closes_the_style_it_opened() {
        for row in [mode_row(), keys_row()] {
            assert!(row.ends_with("\x1b[0m"), "{row:?}");
            assert_eq!(
                row.matches("\x1b[0m").count(),
                row.matches("\x1b[").count() / 2,
                "unbalanced escapes in {row:?}"
            );
        }
    }
}
