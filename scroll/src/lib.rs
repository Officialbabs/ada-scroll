//! A scrolling layout for a Wayland compositor.
//!
//! Windows sit in columns on one horizontal strip that is wider than the
//! screen. A column fills the screen's height and keeps the width the person
//! gives it; windows in one column share its height. When the columns fit on
//! the screen they sit in the middle; when they don't, the strip scrolls
//! sideways to show the focused column.
//!
//! The layout only does arithmetic. The compositor tells it which windows are
//! tiled ([`Strip::add`], [`Strip::remove`]), which one the person picked
//! ([`Strip::focus`]) and what they asked for ([`Strip::apply`]); it reads back
//! where each window goes ([`Strip::placements`]) and moves, sizes and animates
//! the windows itself. Rectangles that lie partly or wholly off the screen are
//! expected: that is the rest of the strip.

/// A rectangle in the compositor's logical coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right() && other.x < self.right() && self.y < other.bottom() && other.y < self.bottom()
    }
}

/// How the strip follows focus once its columns no longer fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// Only as far as it takes to show the whole focused column.
    Edge,
    /// The focused column in the middle, except at either end of the strip.
    Center,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Between columns, between the windows of a column, and around the strip.
    pub gap: i32,
    /// The widths [`Action::NextWidth`] goes through, as shares of the screen.
    /// Columns of shares that add up to 1 fill the screen exactly.
    pub widths: Vec<f64>,
    /// A new column's width, as a share of the screen.
    pub new_width: f64,
    /// How much [`Action::Wider`] and [`Action::Narrower`] change, as a share
    /// of the screen.
    pub width_step: f64,
    pub scroll: Scroll,
    /// Columns that fit on the screen sit in its middle instead of at its left.
    pub center_when_fits: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gap: 12,
            widths: vec![1.0 / 3.0, 0.5, 2.0 / 3.0],
            new_width: 0.5,
            width_step: 1.0 / 12.0,
            scroll: Scroll::Edge,
            center_when_fits: true,
        }
    }
}

/// What the person can ask the layout to do, usually from a shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    FocusFirst,
    FocusLast,
    /// The column at this index, from 0.
    FocusColumn(usize),
    /// Out of its column into a new one on the left, or, alone in its column,
    /// into the column on the left.
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveColumnLeft,
    MoveColumnRight,
    MoveColumnFirst,
    MoveColumnLast,
    NextWidth,
    PreviousWidth,
    Wider,
    Narrower,
    /// The whole screen's width, and back to the width it had.
    FullWidth,
    /// The columns on the screen share it equally and fill it.
    EqualizeVisible,
    /// Only the column's focused window shows, at the column's full height.
    ToggleStacked,
    CenterFocused,
    ScrollLeft,
    ScrollRight,
}

/// Where one window goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement<W> {
    pub window: W,
    pub rect: Rect,
    /// False for the hidden windows of a stacked column; the compositor should
    /// keep them out of sight (and may keep them where `rect` says).
    pub shown: bool,
    pub focused: bool,
}

#[derive(Clone, Debug)]
struct Tile<W> {
    window: W,
    min_width: i32,
}

#[derive(Clone, Debug)]
struct Column<W> {
    tiles: Vec<Tile<W>>,
    active: usize,
    width: i32,
    stacked: bool,
    /// The width before [`Action::FullWidth`].
    restore: Option<i32>,
}

impl<W> Column<W> {
    fn min_width(&self) -> i32 {
        self.tiles.iter().map(|t| t.min_width).max().unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub struct Strip<W> {
    config: Config,
    area: Rect,
    columns: Vec<Column<W>>,
    focus: usize,
    /// The strip's x at the left edge of the view. Negative when the columns
    /// sit in the middle of a screen they don't fill.
    scroll: f64,
}

impl<W: Clone + PartialEq> Strip<W> {
    /// `area` is the part of the screen windows may use (the screen less any
    /// docks and bars).
    pub fn new(config: Config, area: Rect) -> Self {
        Self { config, area, columns: Vec::new(), focus: 0, scroll: 0.0 }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn set_config(&mut self, config: Config) {
        self.config = config;
        for i in 0..self.columns.len() {
            self.clamp_width(i);
        }
        self.reveal();
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    /// A new screen size or usable area. Columns keep their share of the view.
    pub fn set_area(&mut self, area: Rect) {
        let old = self.view_width();
        self.area = area;
        let new = self.view_width();
        if old > 1 && new != old {
            for column in &mut self.columns {
                column.width = scale(column.width, old, new);
                column.restore = column.restore.map(|w| scale(w, old, new));
            }
            for i in 0..self.columns.len() {
                self.clamp_width(i);
            }
        }
        self.reveal();
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn len(&self) -> usize {
        self.columns.iter().map(|c| c.tiles.len()).sum()
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn contains(&self, window: &W) -> bool {
        self.find(window).is_some()
    }

    /// The column and the window's place in it.
    pub fn find(&self, window: &W) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(c, column)| {
            column.tiles.iter().position(|t| t.window == *window).map(|r| (c, r))
        })
    }

    pub fn focused(&self) -> Option<&W> {
        let column = self.columns.get(self.focus)?;
        column.tiles.get(column.active).map(|t| &t.window)
    }

    pub fn focused_column(&self) -> Option<usize> {
        (!self.columns.is_empty()).then_some(self.focus)
    }

    /// The window's column's width.
    pub fn width_of(&self, window: &W) -> Option<i32> {
        self.find(window).map(|(c, _)| self.columns[c].width)
    }

    pub fn is_stacked(&self, window: &W) -> bool {
        self.find(window).is_some_and(|(c, _)| self.columns[c].stacked)
    }

    /// The strip's x at the view's left edge.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Tiles a window in a new column right of the focused one and focuses it.
    pub fn add(&mut self, window: W) {
        if self.contains(&window) {
            return;
        }
        let at = if self.columns.is_empty() { 0 } else { self.focus + 1 };
        let width = self.share_width(self.config.new_width);
        self.columns.insert(at, Column {
            tiles: vec![Tile { window, min_width: 0 }],
            active: 0,
            width,
            stacked: false,
            restore: None,
        });
        self.focus = at;
        self.clamp_width(at);
        self.reveal();
    }

    /// Takes a window off the strip (closed, or now floating). A focused
    /// window's neighbour takes the focus: the next in its column, else the
    /// column that moved into its place, else the one on its left.
    pub fn remove(&mut self, window: &W) -> bool {
        let Some((c, r)) = self.find(window) else {
            return false;
        };
        let column = &mut self.columns[c];
        column.tiles.remove(r);
        if column.tiles.is_empty() {
            self.columns.remove(c);
            if c < self.focus || self.focus >= self.columns.len() {
                self.focus = self.focus.saturating_sub(1);
            }
        } else {
            if r < column.active || column.active >= column.tiles.len() {
                column.active = column.active.saturating_sub(1);
            }
            self.clamp_width(c);
        }
        self.reveal();
        true
    }

    /// The person picked this window (clicked it, or it was activated).
    pub fn focus(&mut self, window: &W) -> bool {
        let Some((c, r)) = self.find(window) else {
            return false;
        };
        self.focus = c;
        self.columns[c].active = r;
        self.reveal();
        true
    }

    /// The smallest width the window can take; its column is never narrower.
    pub fn set_min_width(&mut self, window: &W, min_width: i32) {
        if let Some((c, r)) = self.find(window) {
            self.columns[c].tiles[r].min_width = min_width.max(0);
            self.clamp_width(c);
            self.clamp_scroll();
        }
    }

    /// The window's column takes this width, as when the person drags its
    /// edge. The view stays where it is.
    pub fn set_width(&mut self, window: &W, width: i32) {
        if let Some((c, _)) = self.find(window) {
            self.columns[c].width = width;
            self.columns[c].restore = None;
            self.clamp_width(c);
            self.clamp_scroll();
        }
    }

    /// Moves the view by `dx` (positive shows more of the right), as a
    /// touchpad swipe or wheel would. Focus stays where it is.
    pub fn scroll_by(&mut self, dx: f64) {
        self.scroll += dx;
        self.clamp_scroll();
    }

    pub fn apply(&mut self, action: Action) {
        if self.columns.is_empty() {
            return;
        }
        let last = self.columns.len() - 1;
        let focus = self.focus;
        match action {
            Action::FocusLeft => self.focus = focus.saturating_sub(1),
            Action::FocusRight => self.focus = (focus + 1).min(last),
            Action::FocusUp => {
                let column = &mut self.columns[focus];
                column.active = column.active.saturating_sub(1);
            }
            Action::FocusDown => {
                let column = &mut self.columns[focus];
                column.active = (column.active + 1).min(column.tiles.len() - 1);
            }
            Action::FocusFirst => self.focus = 0,
            Action::FocusLast => self.focus = last,
            Action::FocusColumn(i) => self.focus = i.min(last),
            Action::MoveLeft => self.move_sideways(false),
            Action::MoveRight => self.move_sideways(true),
            Action::MoveUp => {
                let column = &mut self.columns[focus];
                if column.active > 0 {
                    column.tiles.swap(column.active, column.active - 1);
                    column.active -= 1;
                }
            }
            Action::MoveDown => {
                let column = &mut self.columns[focus];
                if column.active + 1 < column.tiles.len() {
                    column.tiles.swap(column.active, column.active + 1);
                    column.active += 1;
                }
            }
            Action::MoveColumnLeft if focus > 0 => {
                self.columns.swap(focus, focus - 1);
                self.focus -= 1;
            }
            Action::MoveColumnRight if focus < last => {
                self.columns.swap(focus, focus + 1);
                self.focus += 1;
            }
            Action::MoveColumnFirst => {
                let column = self.columns.remove(focus);
                self.columns.insert(0, column);
                self.focus = 0;
            }
            Action::MoveColumnLast => {
                let column = self.columns.remove(focus);
                self.columns.push(column);
                self.focus = last;
            }
            Action::MoveColumnLeft | Action::MoveColumnRight => {}
            Action::NextWidth => self.step_preset(true),
            Action::PreviousWidth => self.step_preset(false),
            Action::Wider | Action::Narrower => {
                let step = self.share_step();
                let column = &mut self.columns[focus];
                column.width += if action == Action::Wider { step } else { -step };
                column.restore = None;
                self.clamp_width(focus);
            }
            Action::FullWidth => {
                let full = self.view_width();
                let column = &mut self.columns[focus];
                match column.restore.take() {
                    Some(width) if column.width == full => column.width = width,
                    _ => {
                        column.restore = Some(column.width);
                        column.width = full;
                    }
                }
                self.clamp_width(focus);
            }
            Action::EqualizeVisible => {
                // It keeps its own view; following focus would undo it.
                self.equalize_visible();
                return;
            }
            Action::ToggleStacked => {
                let column = &mut self.columns[focus];
                column.stacked = !column.stacked;
            }
            Action::CenterFocused => {
                let (left, width) = self.span(focus);
                self.scroll = left as f64 + width as f64 / 2.0 - self.view_width() as f64 / 2.0;
                self.clamp_scroll();
                return;
            }
            Action::ScrollLeft | Action::ScrollRight => {
                let third = self.view_width() as f64 / 3.0;
                self.scroll_by(if action == Action::ScrollRight { third } else { -third });
                return;
            }
        }
        self.reveal();
    }

    /// Where every tiled window goes, column by column, top to bottom.
    pub fn placements(&self) -> Vec<Placement<W>> {
        let gap = self.config.gap;
        let top = self.area.y + gap;
        let height = (self.area.h - 2 * gap).max(1);
        let shift = self.scroll.floor() as i32;
        let mut out = Vec::with_capacity(self.len());
        let mut left = 0;
        for (c, column) in self.columns.iter().enumerate() {
            let x = self.area.x + gap + left - shift;
            let shown_rows = if column.stacked { 1 } else { column.tiles.len() as i32 };
            let row_height = (height - (shown_rows - 1) * gap) / shown_rows;
            let spare = height - (shown_rows - 1) * gap - row_height * shown_rows;
            let mut y = top;
            for (r, tile) in column.tiles.iter().enumerate() {
                let rect = if column.stacked {
                    Rect::new(x, top, column.width, height)
                } else {
                    // The last window takes the pixels left by rounding.
                    let h = row_height + if r + 1 == column.tiles.len() { spare } else { 0 };
                    let rect = Rect::new(x, y, column.width, h);
                    y += h + gap;
                    rect
                };
                out.push(Placement {
                    window: tile.window.clone(),
                    rect,
                    shown: !column.stacked || r == column.active,
                    focused: c == self.focus && r == column.active,
                });
            }
            left += column.width + gap;
        }
        out
    }

    /// The width columns can use: the area less the gaps at its two sides.
    pub fn view_width(&self) -> i32 {
        (self.area.w - 2 * self.config.gap).max(1)
    }

    /// The strip's total width, gaps between columns included.
    pub fn total_width(&self) -> i32 {
        let widths: i32 = self.columns.iter().map(|c| c.width).sum();
        widths + self.config.gap * (self.columns.len() as i32 - 1).max(0)
    }

    /// A share of the view in pixels, such that columns whose shares add up to
    /// 1 fill the view exactly with the gaps between them.
    fn share_width(&self, share: f64) -> i32 {
        let gap = self.config.gap as f64;
        (share * (self.view_width() as f64 + gap) - gap).round() as i32
    }

    fn share_step(&self) -> i32 {
        ((self.view_width() as f64 * self.config.width_step).round() as i32).max(1)
    }

    /// The column's left edge on the strip and its width.
    fn span(&self, c: usize) -> (i32, i32) {
        let left: i32 = self.columns[..c].iter().map(|c| c.width + self.config.gap).sum();
        (left, self.columns[c].width)
    }

    fn clamp_width(&mut self, c: usize) {
        let full = self.view_width();
        let column = &mut self.columns[c];
        let least = column.min_width().clamp(1, full);
        column.width = column.width.clamp(least, full);
    }

    fn clamp_scroll(&mut self) {
        let total = self.total_width();
        let view = self.view_width();
        if total <= view {
            self.scroll = if self.config.center_when_fits { (total - view) as f64 / 2.0 } else { 0.0 };
        } else {
            self.scroll = self.scroll.clamp(0.0, (total - view) as f64);
        }
    }

    /// Scrolls to show the focused column.
    fn reveal(&mut self) {
        if self.columns.is_empty() {
            self.focus = 0;
            self.scroll = 0.0;
            return;
        }
        self.focus = self.focus.min(self.columns.len() - 1);
        let view = self.view_width() as f64;
        let (left, width) = self.span(self.focus);
        let (left, right) = (left as f64, (left + width) as f64);
        match self.config.scroll {
            Scroll::Center => self.scroll = (left + right) / 2.0 - view / 2.0,
            Scroll::Edge => {
                if left < self.scroll {
                    self.scroll = left;
                } else if right > self.scroll + view {
                    self.scroll = right - view;
                }
            }
        }
        self.clamp_scroll();
    }

    fn move_sideways(&mut self, right: bool) {
        let c = self.focus;
        let column = &mut self.columns[c];
        if column.tiles.len() > 1 {
            // Out into a column of its own, as wide as the one it left.
            let tile = column.tiles.remove(column.active);
            column.active = column.active.min(column.tiles.len() - 1);
            let width = column.width;
            let at = if right { c + 1 } else { c };
            self.columns.insert(at, Column { tiles: vec![tile], active: 0, width, stacked: false, restore: None });
            self.focus = at;
            self.clamp_width(at);
            if !right {
                self.clamp_width(c + 1);
            } else {
                self.clamp_width(c);
            }
            return;
        }
        // Alone: into the neighbouring column, below its focused window.
        let Some(target) = (if right { c.checked_add(1).filter(|&t| t < self.columns.len()) } else { c.checked_sub(1) })
        else {
            return;
        };
        let tile = self.columns.remove(c).tiles.remove(0);
        let target = if right { target - 1 } else { target };
        let column = &mut self.columns[target];
        column.active += 1;
        column.tiles.insert(column.active, tile);
        self.focus = target;
        self.clamp_width(target);
    }

    fn step_preset(&mut self, forward: bool) {
        let mut widths: Vec<i32> = self.config.widths.iter().map(|&s| self.share_width(s)).collect();
        widths.sort_unstable();
        widths.dedup();
        if widths.is_empty() {
            return;
        }
        let current = self.columns[self.focus].width;
        // Two pixels of slack: a width scaled to a new screen size rounds.
        let next = if forward {
            widths.iter().copied().find(|&w| w > current + 2).unwrap_or(widths[0])
        } else {
            widths.iter().rev().copied().find(|&w| w < current - 2).unwrap_or(*widths.last().unwrap())
        };
        let column = &mut self.columns[self.focus];
        column.width = next;
        column.restore = None;
        self.clamp_width(self.focus);
    }

    fn equalize_visible(&mut self) {
        let view = self.view_width();
        let visible: Vec<usize> = (0..self.columns.len())
            .filter(|&c| {
                let (left, width) = self.span(c);
                let (left, right) = (left as f64, (left + width) as f64);
                // Mostly on the screen.
                let shown = right.min(self.scroll + view as f64) - left.max(self.scroll);
                shown > width as f64 / 2.0
            })
            .collect();
        let (Some(&first), Some(&last)) = (visible.first(), visible.last()) else {
            return;
        };
        let count = (last - first + 1) as i32;
        let width = (view - (count - 1) * self.config.gap) / count;
        for c in first..=last {
            self.columns[c].width = width;
            self.columns[c].restore = None;
            self.clamp_width(c);
        }
        self.scroll = self.span(first).0 as f64;
        self.clamp_scroll();
        if self.focus < first || self.focus > last {
            self.focus = first;
        }
    }
}

fn scale(width: i32, old: i32, new: i32) -> i32 {
    (width as f64 * new as f64 / old as f64).round() as i32
}
