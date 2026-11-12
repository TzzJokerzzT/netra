//! View state, key handling and top-level rendering.
//!
//! [`App`] is deliberately passive and deterministic: every time-related value
//! comes from the [`Snapshot`] the host supplies, so the same keys and the same
//! snapshot always produce the same screen.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use netra_core::domain::{
    DurationMs, ExchangeFilter, ExchangeId, ExchangeSummary, HttpMethod, NetworkExchange, SessionId,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
};

use crate::components;

/// Minimum terminal width the full layout is designed for.
pub const MIN_WIDTH: u16 = 40;
/// Minimum terminal height the full layout is designed for.
pub const MIN_HEIGHT: u16 = 12;
/// From this width on the list and the inspector share the screen.
pub const WIDE_WIDTH: u16 = 100;

/// Header values describing the session currently being inspected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionHeader {
    /// Session identifier, when the session is already persisted.
    pub session_id: Option<SessionId>,
    /// Human-readable session name.
    pub name: String,
    /// Short status label shown in the header, e.g. `capturing`, `paused`, `stopped`.
    pub status: String,
    /// Short source label, e.g. `cdp`, `proxy`, `both`.
    pub sources: String,
    /// Total number of captured requests.
    pub total: u64,
    /// Number of failed, aborted or error-status requests.
    pub errors: u64,
    /// Elapsed capture time, supplied by the host.
    pub elapsed: DurationMs,
}

/// One consistent view of a session, produced by the host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Header describing the session.
    pub header: SessionHeader,
    /// Exchange rows the host wants displayed.
    pub rows: Vec<ExchangeSummary>,
}

/// Full detail of one exchange, fetched on demand by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorData {
    /// The exchange shown in the inspector.
    pub exchange: NetworkExchange,
}

/// Pane that currently owns keyboard navigation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FocusPane {
    /// The request list.
    #[default]
    List,
    /// The inspector tabs.
    Inspector,
}

impl FocusPane {
    /// Short label used in the status line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Inspector => "inspector",
        }
    }
}

/// Tabs of the inspector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InspectorTab {
    /// Method, URL, state, sizes, duration and error insight.
    #[default]
    Overview,
    /// Request headers.
    RequestHeaders,
    /// Response headers.
    ResponseHeaders,
    /// Request payload.
    Payload,
    /// Response payload.
    Response,
    /// Phase timings as a waterfall.
    Timing,
    /// Request cookies and response `Set-Cookie` directives.
    Cookies,
}

impl InspectorTab {
    /// Every tab, in display order.
    pub const ALL: [Self; 7] = [
        Self::Overview,
        Self::RequestHeaders,
        Self::ResponseHeaders,
        Self::Payload,
        Self::Response,
        Self::Timing,
        Self::Cookies,
    ];

    /// Short label used in the tab bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::RequestHeaders => "Req headers",
            Self::ResponseHeaders => "Res headers",
            Self::Payload => "Payload",
            Self::Response => "Response",
            Self::Timing => "Timing",
            Self::Cookies => "Cookies",
        }
    }

    /// One-based position in the tab bar.
    #[must_use]
    pub fn number(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .map_or(1, |index| index + 1)
    }

    /// Maps a one-based tab number to a tab.
    #[must_use]
    pub fn from_number(number: usize) -> Option<Self> {
        number
            .checked_sub(1)
            .and_then(|index| Self::ALL.get(index).copied())
    }

    /// Next tab, or `None` after the last one.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        Self::from_number(self.number() + 1)
    }

    /// Previous tab, or `None` before the first one.
    #[must_use]
    pub fn previous(self) -> Option<Self> {
        self.number().checked_sub(1).and_then(Self::from_number)
    }
}

/// Action a confirmation dialog will emit once accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Delete the current session.
    DeleteSession,
    /// Replay one exchange.
    Replay(ExchangeId),
}

impl ConfirmAction {
    /// Converts the accepted action into the host-facing action.
    #[must_use]
    pub const fn into_action(self) -> AppAction {
        match self {
            Self::DeleteSession => AppAction::DeleteSession,
            Self::Replay(id) => AppAction::Replay(id),
        }
    }
}

/// Pending destructive action waiting for `y`/`n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialog {
    /// Question shown in the dialog.
    pub message: String,
    /// Action emitted when the user confirms.
    pub action: ConfirmAction,
}

/// Intent the host has to act on after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    /// Leave the application.
    Quit,
    /// Persist the current session.
    SaveSession,
    /// Pause or resume capture.
    TogglePause,
    /// The user asked to clear the view (filters/selection/tab). The host need
    /// not act: no stored record is touched.
    ClearView,
    /// The visible filter changed; the host should re-query with [`App::filter`].
    FilterChanged,
    /// Delete the current session (only emitted after confirmation).
    DeleteSession,
    /// Export the session as HAR.
    ExportHar,
    /// Replay one exchange (only emitted after confirmation).
    Replay(ExchangeId),
    /// Re-read the current session.
    Refresh,
}

/// Everything the UI knows and shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    /// Session header values supplied by the host.
    pub header: SessionHeader,
    /// Every exchange row supplied by the host, unfiltered.
    pub rows: Vec<ExchangeSummary>,
    /// Index of the selected row inside the filtered rows.
    pub selection: usize,
    /// First visible row of the filtered list.
    pub scroll: usize,
    /// Pane owning keyboard navigation.
    pub focus: FocusPane,
    /// Active inspector tab.
    pub tab: InspectorTab,
    /// Filter applied to the supplied rows.
    pub filter: ExchangeFilter,
    /// Whether the filter panel overlay is open.
    pub filter_panel_visible: bool,
    /// Buffer of the `/` search input.
    pub search: String,
    /// Text of the filter before the search input was opened.
    pub search_original: String,
    /// Whether the search input owns the keyboard.
    pub search_active: bool,
    /// Whether the help overlay is open.
    pub help_visible: bool,
    /// Whether the attached capture is paused.
    pub paused: bool,
    /// Pending confirmation dialog, if any.
    pub confirm: Option<ConfirmDialog>,
    /// Detail of the selected exchange, when the host supplied it.
    pub detail: Option<InspectorData>,
    /// Status line text.
    pub status: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            header: SessionHeader::default(),
            rows: Vec::new(),
            selection: 0,
            scroll: 0,
            focus: FocusPane::List,
            tab: InspectorTab::Overview,
            filter: ExchangeFilter::default(),
            filter_panel_visible: false,
            search: String::new(),
            search_original: String::new(),
            search_active: false,
            help_visible: false,
            paused: false,
            confirm: None,
            detail: None,
            status: "press ? for help".to_owned(),
        }
    }
}

/// The terminal application: view state plus keyboard handling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct App {
    state: AppState,
}

impl App {
    /// Creates an empty app showing nothing but the help hint.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the header and the supplied rows, keeping the view state.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.state.header = snapshot.header;
        self.state.rows = snapshot.rows;
        self.clamp_selection();
    }

    /// Stores the detail of the selected exchange, or clears it.
    pub fn set_detail(&mut self, detail: Option<InspectorData>) {
        self.state.detail = detail;
    }

    /// Replaces the status line text.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.state.status = message.into();
    }

    /// Records whether the attached capture is paused.
    pub fn set_paused(&mut self, paused: bool) {
        self.state.paused = paused;
    }

    /// Replaces the filter and clamps the selection to the new result set.
    pub fn set_filter(&mut self, filter: ExchangeFilter) {
        self.state.filter = filter;
        self.clamp_selection();
    }

    /// Current view state.
    #[must_use]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Filter the host should re-query with.
    #[must_use]
    pub fn filter(&self) -> &ExchangeFilter {
        &self.state.filter
    }

    /// Rows that survive the current filter, in supplied order.
    #[must_use]
    pub fn visible_rows(&self) -> Vec<&ExchangeSummary> {
        self.state
            .rows
            .iter()
            .filter(|row| self.state.filter.matches(row))
            .collect()
    }

    /// Identifier of the selected row, if any.
    #[must_use]
    pub fn selected_id(&self) -> Option<ExchangeId> {
        self.visible_rows()
            .get(self.state.selection)
            .map(|row| row.id)
    }

    /// `true` when the host must call [`App::set_detail`] for the current selection.
    #[must_use]
    pub fn needs_detail(&self) -> bool {
        match self.selected_id() {
            Some(id) => self.state.detail.as_ref().map(|detail| detail.exchange.id) != Some(id),
            None => false,
        }
    }

    /// Handles one key press, returning the action the host must perform.
    ///
    /// A key never fires a shortcut while the search input or a confirmation
    /// dialog owns the keyboard.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if self.state.confirm.is_some() {
            return self.resolve_confirm(&key);
        }
        if self.state.search_active {
            return self.edit_search(&key);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        match key.code {
            KeyCode::Char('q') => Some(AppAction::Quit),
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(true);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(false);
                None
            }
            KeyCode::Char('g') => {
                self.state.selection = 0;
                None
            }
            KeyCode::Char('G') => {
                self.state.selection = self.visible_rows().len().saturating_sub(1);
                None
            }
            KeyCode::Enter => {
                self.state.focus = FocusPane::Inspector;
                None
            }
            KeyCode::Tab => {
                self.cycle_focus(true);
                None
            }
            KeyCode::BackTab => {
                self.cycle_focus(false);
                None
            }
            KeyCode::Char(digit) if digit.is_ascii_digit() => {
                let number = digit
                    .to_digit(10)
                    .and_then(|value| usize::try_from(value).ok());
                if let Some(tab) = number.and_then(InspectorTab::from_number) {
                    self.state.tab = tab;
                    self.state.focus = FocusPane::Inspector;
                }
                None
            }
            KeyCode::Char('/') => {
                self.open_search();
                None
            }
            KeyCode::Char('f') => {
                self.state.filter_panel_visible = !self.state.filter_panel_visible;
                None
            }
            KeyCode::Char('F') => {
                self.state.filter.only_failed = !self.state.filter.only_failed;
                self.clamp_selection();
                Some(AppAction::FilterChanged)
            }
            KeyCode::Char('s') => Some(AppAction::SaveSession),
            KeyCode::Char('p') => Some(AppAction::TogglePause),
            KeyCode::Char('c') => {
                self.clear_view();
                Some(AppAction::ClearView)
            }
            KeyCode::Char('r') => {
                self.open_replay_confirm();
                None
            }
            KeyCode::Char('d') => {
                self.state.confirm = Some(ConfirmDialog {
                    message: "Delete this session and every exchange stored in it?".to_owned(),
                    action: ConfirmAction::DeleteSession,
                });
                None
            }
            KeyCode::Char('e') => Some(AppAction::ExportHar),
            KeyCode::Char('?') => {
                self.state.help_visible = !self.state.help_visible;
                None
            }
            KeyCode::Esc => {
                self.close_overlay();
                None
            }
            _ => None,
        }
    }

    /// Renders the whole screen.
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            components::too_small::render(frame, area);
            return;
        }
        let rows = self.visible_rows();
        let selected = self.state.selection;
        let wide = area.width >= WIDE_WIDTH;
        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
        let (header_area, body_area, status_area) = (layout[0], layout[1], layout[2]);

        let mut header = self.state.header.clone();
        if self.state.paused {
            header.status = "paused".to_owned();
        }
        components::header::render(frame, header_area, &header);
        let list_area = match (wide, self.state.focus) {
            (true, _) => {
                let panes =
                    Layout::horizontal([Constraint::Percentage(64), Constraint::Percentage(36)])
                        .split(body_area);
                components::list::render(
                    frame,
                    panes[0],
                    &rows,
                    selected,
                    self.state.scroll,
                    self.state.focus == FocusPane::List,
                );
                components::inspector::render(
                    frame,
                    panes[1],
                    self.state.detail.as_ref(),
                    self.state.tab,
                );
                Some(panes[0])
            }
            (false, FocusPane::Inspector) => {
                components::inspector::render(
                    frame,
                    body_area,
                    self.state.detail.as_ref(),
                    self.state.tab,
                );
                None
            }
            (false, FocusPane::List) => {
                components::list::render(
                    frame,
                    body_area,
                    &rows,
                    selected,
                    self.state.scroll,
                    true,
                );
                Some(body_area)
            }
        };
        self.update_scroll(rows.len(), list_area);
        components::status::render(frame, status_area, &self.status_line());

        if self.state.help_visible {
            components::help::render(frame, area);
        }
        if self.state.filter_panel_visible && self.state.confirm.is_none() {
            components::filter_panel::render(frame, area, &self.state);
        }
        if let Some(dialog) = &self.state.confirm {
            components::confirm::render(frame, area, dialog);
        }
    }

    /// Status line text, including the search prompt while it is focused.
    fn status_line(&self) -> String {
        if self.state.search_active {
            return format!("Search: {}   (Enter apply · Esc cancel)", self.state.search);
        }
        let visible = self.visible_rows().len();
        let total = self.state.rows.len();
        let mut parts = vec![format!(
            "{visible}/{total} shown · {}",
            self.state.focus.label()
        )];
        if !self.state.filter.is_empty() {
            parts.push(self.filter_summary());
        }
        if !self.state.status.is_empty() {
            parts.push(self.state.status.clone());
        }
        parts.push("? help".to_owned());
        parts.join(" · ")
    }

    /// Compact description of the active filter.
    fn filter_summary(&self) -> String {
        let filter = &self.state.filter;
        let mut parts: Vec<String> = Vec::new();
        if let Some(text) = filter.text.as_deref().filter(|text| !text.is_empty()) {
            parts.push(format!("text={text}"));
        }
        if let Some(host) = filter.host.as_deref().filter(|host| !host.is_empty()) {
            parts.push(format!("host={host}"));
        }
        if !filter.methods.is_empty() {
            parts.push(format!("methods={}", methods_text(&filter.methods)));
        }
        if !filter.status.is_empty() {
            parts.push(format!("status={}", filter.status.to_text()));
        }
        if !filter.media_kinds.is_empty() {
            parts.push(format!(
                "kind={}",
                filter
                    .media_kinds
                    .iter()
                    .map(|kind| kind.label())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if !filter.sources.is_empty() {
            parts.push(format!(
                "src={}",
                filter
                    .sources
                    .iter()
                    .map(|source| source.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if !filter.states.is_empty() {
            parts.push(format!(
                "state={}",
                filter
                    .states
                    .iter()
                    .map(|state| state.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if filter.only_failed {
            parts.push("only-failed".to_owned());
        }
        format!("filter[{}]", parts.join(" "))
    }

    /// Keeps the selected row inside the filtered list.
    fn clamp_selection(&mut self) {
        let count = self.visible_rows().len();
        if count == 0 {
            self.state.selection = 0;
            self.state.scroll = 0;
            return;
        }
        self.state.selection = self.state.selection.min(count - 1);
    }

    /// Moves the selection by one row, clamped to the filtered list.
    fn move_selection(&mut self, forward: bool) {
        let count = self.visible_rows().len();
        if count == 0 {
            self.state.selection = 0;
            return;
        }
        let last = count - 1;
        let current = self.state.selection.min(last);
        self.state.selection = if forward {
            current.saturating_add(1).min(last)
        } else {
            current.saturating_sub(1)
        };
    }

    /// Cycles `List -> Inspector(tabs) -> List`, forwards or backwards.
    fn cycle_focus(&mut self, forward: bool) {
        match (self.state.focus, forward) {
            (FocusPane::List, true) => self.state.focus = FocusPane::Inspector,
            (FocusPane::List, false) => {
                self.state.focus = FocusPane::Inspector;
                self.state.tab = InspectorTab::Cookies;
            }
            (FocusPane::Inspector, true) => match self.state.tab.next() {
                Some(tab) => self.state.tab = tab,
                None => {
                    self.state.tab = InspectorTab::Overview;
                    self.state.focus = FocusPane::List;
                }
            },
            (FocusPane::Inspector, false) => match self.state.tab.previous() {
                Some(tab) => self.state.tab = tab,
                None => self.state.focus = FocusPane::List,
            },
        }
    }

    /// Opens the `/` search input, seeded with the current text filter.
    fn open_search(&mut self) {
        let current = self.state.filter.text.clone().unwrap_or_default();
        self.state.search_original = current.clone();
        self.state.search = current;
        self.state.search_active = true;
        self.state.status = "editing text filter".to_owned();
    }

    /// Applies the search buffer to the text filter and re-clamps the selection.
    fn apply_search_text(&mut self) {
        let text = self.state.search.trim().to_owned();
        self.state.filter.text = if text.is_empty() { None } else { Some(text) };
        self.clamp_selection();
    }

    /// Handles keys while the search input owns the keyboard.
    fn edit_search(&mut self, key: &KeyEvent) -> Option<AppAction> {
        match key.code {
            KeyCode::Esc => {
                self.state.search = self.state.search_original.clone();
                self.state.search_active = false;
                self.apply_search_text();
                self.state.status = "search cancelled".to_owned();
                Some(AppAction::FilterChanged)
            }
            KeyCode::Enter => {
                self.state.search_active = false;
                self.apply_search_text();
                self.state.status = "text filter applied".to_owned();
                Some(AppAction::FilterChanged)
            }
            KeyCode::Backspace => {
                if self.state.search.pop().is_some() {
                    self.apply_search_text();
                    return Some(AppAction::FilterChanged);
                }
                None
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.state.search.push(character);
                self.apply_search_text();
                Some(AppAction::FilterChanged)
            }
            _ => None,
        }
    }

    /// Opens the replay dialog for the selected exchange.
    fn open_replay_confirm(&mut self) {
        match self.selected_id() {
            Some(id) => {
                self.state.confirm = Some(ConfirmDialog {
                    message: format!("Replay exchange {} as a new request?", id.short()),
                    action: ConfirmAction::Replay(id),
                });
            }
            None => {
                self.state.status = "no exchange selected to replay".to_owned();
            }
        }
    }

    /// Resolves the pending confirmation dialog.
    fn resolve_confirm(&mut self, key: &KeyEvent) -> Option<AppAction> {
        let dialog = self.state.confirm.clone()?;
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                self.state.confirm = None;
                self.state.status = "confirmed".to_owned();
                Some(dialog.action.into_action())
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.state.confirm = None;
                self.state.status = "cancelled".to_owned();
                None
            }
            _ => None,
        }
    }

    /// Closes the topmost overlay.
    fn close_overlay(&mut self) {
        if self.state.help_visible {
            self.state.help_visible = false;
        } else if self.state.filter_panel_visible {
            self.state.filter_panel_visible = false;
        }
    }

    /// Resets filters, search, selection, tab and focus. Rows are never touched.
    fn clear_view(&mut self) {
        self.state.filter = ExchangeFilter::default();
        self.state.search.clear();
        self.state.search_original.clear();
        self.state.search_active = false;
        self.state.filter_panel_visible = false;
        self.state.selection = 0;
        self.state.scroll = 0;
        self.state.tab = InspectorTab::Overview;
        self.state.focus = FocusPane::List;
        self.state.status = "view cleared (stored records untouched)".to_owned();
    }

    /// Scrolls the list just enough to keep the selection visible.
    fn update_scroll(&mut self, row_count: usize, list_area: Option<Rect>) {
        let Some(list_area) = list_area else {
            self.state.scroll = 0;
            return;
        };
        let inner_height = usize::from(list_area.height.saturating_sub(1));
        if inner_height == 0 || row_count == 0 {
            self.state.scroll = 0;
            return;
        }
        let selection = self.state.selection.min(row_count - 1);
        let mut scroll = self.state.scroll;
        if selection < scroll {
            scroll = selection;
        } else if selection >= scroll.saturating_add(inner_height) {
            scroll = selection.saturating_add(1).saturating_sub(inner_height);
        }
        self.state.scroll = scroll.min(row_count.saturating_sub(inner_height.min(row_count)));
    }
}

/// Comma-separated method names.
fn methods_text(methods: &[HttpMethod]) -> String {
    methods
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_cycle_forward_and_backward() {
        assert_eq!(
            InspectorTab::Overview.next(),
            Some(InspectorTab::RequestHeaders)
        );
        assert_eq!(InspectorTab::Overview.previous(), None);
        assert_eq!(InspectorTab::Cookies.next(), None);
        assert_eq!(InspectorTab::Timing.number(), 6);
        assert_eq!(InspectorTab::from_number(7), Some(InspectorTab::Cookies));
        assert_eq!(InspectorTab::from_number(0), None);
        assert_eq!(InspectorTab::from_number(8), None);
    }

    #[test]
    fn confirm_actions_map_to_host_actions() {
        let id = ExchangeId::new();
        assert_eq!(
            ConfirmAction::Replay(id).into_action(),
            AppAction::Replay(id)
        );
        assert_eq!(
            ConfirmAction::DeleteSession.into_action(),
            AppAction::DeleteSession
        );
    }

    #[test]
    fn release_events_are_ignored() {
        let mut app = App::new();
        let key = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(app.on_key(key), None);
    }
}
