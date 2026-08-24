use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use serde::Deserialize;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub mod herdr;

const MODE_TAG: &str = "-- NORMAL --";

/// A workspace as the sidebar shows it, in the order `workspace list` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
}

/// An agent pane as the sidebar shows it, in the order `agent list` returns.
/// The owning workspace is deliberately absent: the rows are one flat list in
/// the order the CLI gave them, so nothing groups or filters by workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane_id: String,
    pub label: String,
}

/// One selectable sidebar line. Spaces come first, then agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Space { workspace_id: String, label: String },
    Agent { pane_id: String, label: String },
}

/// What a keystroke means. Deliberately free of side effects: the caller owns
/// the herdr calls so the key handling stays testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Move(isize),
    Quit,
    Ignore,
}

impl Row {
    pub fn label(&self) -> &str {
        match self {
            Row::Space { label, .. } | Row::Agent { label, .. } => label,
        }
    }
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
    label: Option<String>,
    #[serde(default)]
    number: Option<u64>,
}

#[derive(Deserialize)]
struct AgentListResult {
    #[serde(default)]
    agents: Vec<RawAgent>,
}

/// Only `pane_id` is required. Everything else is missing on at least one of
/// the agents a real `herdr agent list` returns.
#[derive(Deserialize)]
struct RawAgent {
    #[serde(default)]
    pane_id: Option<String>,
    #[serde(default)]
    terminal_title_stripped: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    agent: Option<String>,
}

/// A present-but-blank field carries no more information than a missing one,
/// so both collapse to `None`.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Pick the first candidate that is present and not blank.
fn first_non_empty(candidates: &[Option<&str>], fallback: &str) -> String {
    candidates
        .iter()
        .copied()
        .find_map(non_blank)
        .unwrap_or(fallback)
        .to_string()
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
            let workspace_id = raw.workspace_id?;
            let number = raw.number.map(|n| n.to_string());
            let label = first_non_empty(&[raw.label.as_deref(), number.as_deref()], &workspace_id);
            Some(Workspace {
                workspace_id,
                label,
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
            let pane_id = raw.pane_id?;
            let label = first_non_empty(
                &[
                    raw.terminal_title_stripped.as_deref(),
                    raw.name.as_deref(),
                    raw.agent.as_deref(),
                ],
                &pane_id,
            );
            Some(Agent { pane_id, label })
        })
        .collect()
}

/// One continuous list matching the sidebar: the spaces section, then the
/// agents section. Both keep the order the CLI returned them in.
pub fn build_rows(workspaces: &[Workspace], agents: &[Agent], own_pane_id: Option<&str>) -> Vec<Row> {
    let spaces = workspaces.iter().map(|workspace| Row::Space {
        workspace_id: workspace.workspace_id.clone(),
        label: workspace.label.clone(),
    });
    let agents = agents
        .iter()
        .filter(|agent| Some(agent.pane_id.as_str()) != own_pane_id)
        .map(|agent| Row::Agent {
            pane_id: agent.pane_id.clone(),
            label: agent.label.clone(),
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
        KeyCode::Char('i') | KeyCode::Enter if bare => Action::Quit,
        _ => Action::Ignore,
    }
}

/// Cut a string down to `max` display columns, marking the cut with an
/// ellipsis. Counting columns rather than bytes keeps CJK titles from either
/// overflowing the popup or splitting a character.
fn truncate_to_width(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1;
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width > budget {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

/// A one-line message in place of the list. herdr tears the popup down the
/// moment the process exits, so anything written to stderr is gone before it
/// can be read — the only place a failure can be reported is here.
pub fn render_notice(message: &str, width: usize) -> String {
    let fixed = MODE_TAG.width() + 2;
    if fixed >= width {
        return truncate_to_width(MODE_TAG, width);
    }
    format!("{MODE_TAG}  {}", truncate_to_width(message, width - fixed))
}

pub fn render_line(rows: &[Row], cursor: usize, width: usize) -> String {
    if rows.is_empty() {
        return truncate_to_width(MODE_TAG, width);
    }
    let index = cursor.min(rows.len() - 1);
    let position = format!("{}/{}", index + 1, rows.len());
    // Two spaces separate the three parts.
    let fixed = MODE_TAG.width() + position.width() + 2;
    if fixed >= width {
        return truncate_to_width(MODE_TAG, width);
    }
    let label = truncate_to_width(rows[index].label(), width - fixed);
    format!("{MODE_TAG} {label} {position}")
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

    /// Trimmed from a real `herdr agent list` response. The second agent is the
    /// only one carrying `name`, and the third has no `name` at all — the shape
    /// really does vary per agent.
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

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn space(id: &str, label: &str) -> Row {
        Row::Space {
            workspace_id: id.into(),
            label: label.into(),
        }
    }

    fn agent_row(id: &str, label: &str) -> Row {
        Row::Agent {
            pane_id: id.into(),
            label: label.into(),
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
                Workspace { workspace_id: "w7".into(), label: "tools".into() },
                Workspace { workspace_id: "w9".into(), label: "work".into() },
                Workspace { workspace_id: "wB".into(), label: "personal".into() },
            ]
        );
    }

    #[test]
    fn workspace_without_label_falls_back_to_number_then_id() {
        let json = r#"{"result":{"workspaces":[
            {"workspace_id":"w1","number":4},
            {"workspace_id":"w2"}
        ]}}"#;

        let workspaces = parse_workspace_list(json);

        assert_eq!(workspaces[0].label, "4");
        assert_eq!(workspaces[1].label, "w2");
    }

    #[test]
    fn parses_agents_in_cli_order_with_title_preferred() {
        let agents = parse_agent_list(AGENT_LIST_JSON);

        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0].pane_id, "w9:p9P");
        assert_eq!(agents[0].label, "各種タスクのスケジュール調整");
        // `name` is present here but the stripped title still wins.
        assert_eq!(agents[1].label, "fix-login-redirect");
        assert_eq!(agents[2].pane_id, "wB:p2");
    }

    #[test]
    fn agent_label_falls_back_through_name_agent_and_pane_id() {
        let json = r#"{"result":{"agents":[
            {"pane_id":"w1:p1","workspace_id":"w1","name":"builder","agent":"claude"},
            {"pane_id":"w1:p2","workspace_id":"w1","agent":"codex"},
            {"pane_id":"w1:p3","workspace_id":"w1"},
            {"pane_id":"w1:p4","workspace_id":"w1","terminal_title_stripped":""}
        ]}}"#;

        let agents = parse_agent_list(json);

        assert_eq!(agents[0].label, "builder");
        assert_eq!(agents[1].label, "codex");
        assert_eq!(agents[2].label, "w1:p3");
        // An empty title is as useless as a missing one.
        assert_eq!(agents[3].label, "w1:p4");
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

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].label, "real");
    }

    #[test]
    fn workspace_without_an_id_is_dropped() {
        let json = r#"{"result":{"workspaces":[
            {"label":"ghost"},
            {"workspace_id":"w1","label":"real"}
        ]}}"#;

        let workspaces = parse_workspace_list(json);

        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].label, "real");
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
                space("w7", "tools"),
                space("w9", "work"),
                space("wB", "personal"),
                agent_row("w9:p9P", "各種タスクのスケジュール調整"),
                agent_row("w9:pEV", "fix-login-redirect"),
                agent_row("wB:p2", "vim-normal"),
            ]
        );
    }

    #[test]
    fn our_own_pane_is_left_out_of_the_rows() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST_JSON);
        let agents = parse_agent_list(AGENT_LIST_JSON);

        let rows = build_rows(&workspaces, &agents, Some("w9:pEV"));

        assert_eq!(rows.len(), 5);
        assert!(!rows.contains(&agent_row("w9:pEV", "fix-login-redirect")));
    }

    #[test]
    fn rows_hold_up_with_no_agents_at_all() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST_JSON);

        let rows = build_rows(&workspaces, &[], None);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], space("w7", "tools"));
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

    #[test]
    fn rendered_line_names_the_mode_the_row_and_the_position() {
        let rows = space_rows();

        let line = render_line(&rows, 1, 40);

        assert!(line.starts_with("-- NORMAL --"), "{line}");
        assert!(line.contains("work"), "{line}");
        assert!(line.contains("2/3"), "{line}");
    }

    #[test]
    fn rendered_line_fits_the_popup_even_with_wide_characters() {
        let rows = full_rows();

        for width in [10, 20, 26, 40] {
            for cursor in 0..rows.len() {
                let line = render_line(&rows, cursor, width);
                assert!(
                    line.width() <= width,
                    "width {width} cursor {cursor}: {line:?} is {} columns",
                    line.width()
                );
            }
        }
    }

    #[test]
    fn a_notice_carries_the_mode_tag_and_the_message() {
        let line = render_notice("no rows", 40);

        assert!(line.starts_with("-- NORMAL --"), "{line}");
        assert!(line.contains("no rows"), "{line}");
    }

    #[test]
    fn a_notice_fits_the_popup() {
        for width in [8, 12, 20, 37, 40] {
            let line = render_notice("herdr did not answer 各種タスク", width);

            assert!(
                line.width() <= width,
                "width {width}: {line:?} is {} columns",
                line.width()
            );
        }
    }

    #[test]
    fn rendering_an_empty_list_does_not_panic() {
        assert!(render_line(&[], 0, 40).contains("NORMAL"));
    }

    #[test]
    fn rendering_an_out_of_range_cursor_does_not_panic() {
        let rows = space_rows();

        let line = render_line(&rows, 99, 40);

        assert!(line.starts_with("-- NORMAL --"), "{line}");
    }
}
