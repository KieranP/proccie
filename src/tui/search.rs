//! The log-search box state: the query, its edit cursor, and whether the box is
//! open for editing (vs a committed filter that still applies with the box shut).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A search over the active tab. `editing` distinguishes the open, key-capturing
/// box from a committed filter; either way a non-empty `query` filters the view.
#[derive(Hash)]
pub struct Search {
    query: String,
    /// Cursor byte offset into `query`; always on a char boundary.
    cursor: usize,
    editing: bool,
}

/// What [`Search::edit`] asks the caller (the app) to do after a key.
pub enum SearchAction {
    /// Not an editing key — fall through to normal handling (scroll, tab switch).
    Pass,
    /// The query changed; re-follow the newest matches.
    Refilter,
    /// Handled with no query change (cursor moved, or committed); leave scroll be.
    Handled,
    /// Close the box and clear the filter.
    Close,
}

impl Search {
    /// Opens editing on `query` (empty for a fresh box), cursor at the end.
    pub fn editing(query: String) -> Search {
        Search {
            cursor: query.len(),
            query,
            editing: true,
        }
    }

    /// The active filter: the query only while it is non-empty.
    pub fn filter_query(&self) -> Option<&str> {
        Some(self.query.as_str()).filter(|q| !q.is_empty())
    }

    /// Whether the box is open and capturing keys.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// The query text (for the footer).
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The edit cursor's byte offset (for drawing the caret).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Consumes the search, yielding its query (to reopen editing on it).
    pub fn into_query(self) -> String {
        self.query
    }

    /// Applies one editing key, mutating the query/cursor, and reports what the
    /// caller should do next. Non-editing keys return [`SearchAction::Pass`].
    pub fn edit(&mut self, key: KeyEvent) -> SearchAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let cmd = key.modifiers.contains(KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => SearchAction::Close,
            KeyCode::Enter => self.commit(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            // Cmd/Super + arrow, or the readline Ctrl+A/Ctrl+E, jumps to an end.
            KeyCode::Left if cmd => self.cursor_home(),
            KeyCode::Right if cmd => self.cursor_end(),
            KeyCode::Char('a') if ctrl => self.cursor_home(),
            KeyCode::Char('e') if ctrl => self.cursor_end(),
            KeyCode::Left => self.cursor_left(),
            KeyCode::Right => self.cursor_right(),
            KeyCode::Home => self.cursor_home(),
            KeyCode::End => self.cursor_end(),
            // A bare printable char (no command modifier) extends the query.
            KeyCode::Char(c) if !ctrl && !alt && !cmd => self.insert(c),
            _ => SearchAction::Pass,
        }
    }

    /// Enter: keep a non-empty query as a committed filter; an empty one closes.
    fn commit(&mut self) -> SearchAction {
        if self.query.is_empty() {
            SearchAction::Close
        } else {
            self.editing = false;
            SearchAction::Handled
        }
    }

    /// Inserts `c` at the cursor and steps past it.
    fn insert(&mut self, c: char) -> SearchAction {
        self.query.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        SearchAction::Refilter
    }

    /// Deletes the char before the cursor (a no-op at the start).
    fn backspace(&mut self) -> SearchAction {
        if let Some(prev) = self.query[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
            self.query.remove(self.cursor);
        }
        SearchAction::Refilter
    }

    /// Deletes the char at the cursor (a no-op at the end).
    fn delete(&mut self) -> SearchAction {
        if self.cursor < self.query.len() {
            self.query.remove(self.cursor);
        }
        SearchAction::Refilter
    }

    /// Moves the cursor one char left.
    fn cursor_left(&mut self) -> SearchAction {
        if let Some(prev) = self.query[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
        SearchAction::Handled
    }

    /// Moves the cursor one char right.
    fn cursor_right(&mut self) -> SearchAction {
        if let Some(next) = self.query[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
        SearchAction::Handled
    }

    /// Jumps the cursor to the start of the query.
    fn cursor_home(&mut self) -> SearchAction {
        self.cursor = 0;
        SearchAction::Handled
    }

    /// Jumps the cursor to the end of the query.
    fn cursor_end(&mut self) -> SearchAction {
        self.cursor = self.query.len();
        SearchAction::Handled
    }
}
