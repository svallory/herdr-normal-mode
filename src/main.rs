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
    Action, Row, build_rows, handle_key, initial_cursor, move_cursor, render_line, render_notice,
    resolve_active_ids,
};

/// Last resort when the terminal will not say how wide it is. The manifest's
/// `width = 40` is the *outer* popup size — herdr spends two columns on the
/// border and one more inside it — so the pane itself is 37 columns. Writing
/// 40 here would wrap the line and push the mode row off the popup.
const FALLBACK_WIDTH: usize = 37;

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

    let result = if rows.is_empty() {
        // Nothing to walk: either herdr has no workspaces, or it never
        // answered. Say so instead of flashing the popup shut.
        notice("nothing to move through")
    } else {
        let start =
            initial_cursor(&rows, active_pane_id.as_deref(), active_workspace_id.as_deref());
        // Bind the result before tearing down, so no `?` can skip past the
        // restore and strand the terminal in raw mode.
        run(&herdr, &rows, start)
    };
    let _ = restore();

    if let Err(err) = result {
        let _ = notice(&format!("failed: {err}"));
        let _ = restore();
    }
}

fn run(herdr: &str, rows: &[Row], start: Option<usize>) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    // No alternate screen: the popup is its own surface and gets torn down
    // with us.
    print!("\x1b[?25l\x1b[2J\x1b[H");

    let mut cursor = start.unwrap_or(0);
    draw(rows, cursor)?;

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
                    cursor = move_cursor(cursor, rows.len(), delta);
                    // The cursor tracks the keystroke without waiting on
                    // herdr; the focus catches up at the settle above.
                    draw(rows, cursor)?;
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
            Event::Resize(..) => draw(rows, cursor)?,
            _ => {}
        }
    }

    Ok(())
}

/// Ask the pane how wide it is rather than trusting the manifest, which
/// measures the popup from the outside of its border.
fn viewport_width() -> usize {
    terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(FALLBACK_WIDTH)
}

fn draw(rows: &[Row], cursor: usize) -> io::Result<()> {
    write_line(&render_line(rows, cursor, viewport_width()))
}

/// Put a message on the popup's one line and hold it there until a keystroke.
/// The popup dies with this process, so there is nowhere else to report to.
fn notice(message: &str) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    print!("\x1b[?25l\x1b[2J\x1b[H");
    write_line(&render_notice(message, viewport_width()))?;
    let _ = event::read();
    Ok(())
}

fn write_line(line: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    write!(stdout, "\x1b[H\x1b[2K{line}")?;
    stdout.flush()
}

fn restore() -> io::Result<()> {
    print!("\x1b[?25h");
    io::stdout().flush()?;
    terminal::disable_raw_mode()
}
