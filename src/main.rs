//! A vim-like normal mode for the herdr sidebar: `j` and `k` walk the spaces
//! and agents as one list, and herdr's focus follows the cursor.
//!
//! This runs as a session-modal popup. Any other placement would be pushed
//! behind the pane we just focused, taking our input and our drawing with it.

use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::terminal;

use herdr_normal_mode::herdr::{fetch_lists, focus_row, herdr_bin};
use herdr_normal_mode::{
    Action, Row, build_rows, handle_key, initial_cursor, keys_row, mode_row, move_cursor,
    resolve_active_ids,
};

fn main() {
    let herdr = herdr_bin();

    let context_json = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok();
    // No pane fallback on purpose: `HERDR_PANE_ID` is this popup's own id.
    let env_workspace_id = std::env::var("HERDR_WORKSPACE_ID").ok();
    let (active_pane_id, active_workspace_id) =
        resolve_active_ids(context_json.as_deref(), env_workspace_id.as_deref());
    let own_pane_id = std::env::var("HERDR_PANE_ID").ok();

    let (workspaces, agents) = fetch_lists(&herdr);
    let rows = build_rows(&workspaces, &agents, own_pane_id.as_deref());

    // Nothing to walk means herdr answered with no workspaces at all, which it
    // cannot do while we are running inside one of them. Close again rather
    // than carry a reporting path for it.
    if rows.is_empty() {
        return;
    }

    let start = initial_cursor(&rows, active_pane_id.as_deref(), active_workspace_id.as_deref());
    // Nothing is done with a failure. Every error `run` can return comes from
    // the terminal itself, which is the only place a report could go, and herdr
    // tears the popup down the moment we exit — so there is nowhere to say it
    // and nothing the user could do about it. Dropping it here is also what
    // keeps a `?` inside `run` from skipping the restore below and stranding
    // the terminal in raw mode.
    let _ = run(&herdr, &rows, start);
    let _ = restore();
}

fn run(herdr: &str, rows: &[Row], start: Option<usize>) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    // No alternate screen: the popup is its own surface and gets torn down
    // with us.
    print!("\x1b[?25l\x1b[2J\x1b[H");

    let mut cursor = start.unwrap_or(0);
    // Written once and then left alone: the rows say the mode is on and how to
    // work it, neither of which a keystroke changes. Where the cursor is shows
    // in the sidebar, which the focus below moves for us.
    write_rows()?;

    // The row herdr is focused on, as far as we know. `None` when the cursor
    // started somewhere we could not match, so the first settle really sends a
    // focus. We never re-read herdr's own `focused` flags — following our own
    // focus changes would be a loop.
    let mut followed = start;

    loop {
        // Settle the focus once the burst is over. Holding `j` queues keys
        // faster than herdr can answer them, so a burst costs one round trip
        // at the row it lands on instead of one per keystroke. Doing this at
        // the top of the loop means every path back here settles, not just
        // the one that moved the cursor.
        if followed != Some(cursor)
            && !event::poll(Duration::ZERO)?
            // Only count the row as reached if herdr accepted it. A pane that
            // died while the popup was open should be retried, not written off.
            && focus_row(herdr, &rows[cursor])
        {
            followed = Some(cursor);
        }

        match event::read()? {
            Event::Key(key) => match handle_key(&key) {
                Action::Move(delta) => {
                    // The cursor tracks the keystroke without waiting on
                    // herdr; the focus catches up at the settle above.
                    cursor = move_cursor(cursor, rows.len(), delta);
                }
                Action::Quit => {
                    // A burst that ended in a quit never reached the settle
                    // above; land on the row the cursor stopped on.
                    if followed != Some(cursor) {
                        focus_row(herdr, &rows[cursor]);
                    }
                    break;
                }
                Action::Ignore => {}
            },
            // A resize can clear the pane out from under us, so put the rows
            // back.
            Event::Resize(..) => write_rows()?,
            _ => {}
        }
    }

    Ok(())
}

/// Both rows, each placed by absolute position. A newline would do instead, but
/// only until the pane is exactly as tall as what we draw — then the last one
/// scrolls the first row away.
fn write_rows() -> io::Result<()> {
    let mut stdout = io::stdout();
    write!(
        stdout,
        "\x1b[1;1H\x1b[2K{}\x1b[2;1H\x1b[2K{}",
        mode_row(),
        keys_row()
    )?;
    stdout.flush()
}

fn restore() -> io::Result<()> {
    print!("\x1b[?25h");
    io::stdout().flush()?;
    terminal::disable_raw_mode()
}
