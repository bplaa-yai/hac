use hac_core::net::request_manager::Response;
use hac_core::syntax::highlighter::HIGHLIGHTER;

use crate::ascii::{BIG_ERROR_ARTS, LOGO_ASCII, SMALL_ERROR_ARTS};
use crate::pages::collection_viewer::collection_viewer::PaneFocus;
use crate::pages::under_construction::UnderConstruction;
use crate::pages::{spinner::Spinner, Eventful, Renderable};
use crate::utils::build_syntax_highlighted_lines;

use std::cell::RefCell;
use std::iter;
use std::ops::{Add, Sub};
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rand::Rng;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Scrollbar};
use ratatui::widgets::{ScrollbarOrientation, ScrollbarState, Tabs};
use ratatui::Frame;
use tree_sitter::Tree;

use super::collection_store::CollectionStore;

#[derive(Debug)]
pub enum ResponseViewerEvent {
    RemoveSelection,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResViewerTabs {
    Preview,
    Raw,
    Cookies,
    Headers,
}

impl ResViewerTabs {
    pub fn next(tab: &ResViewerTabs) -> Self {
        match tab {
            Self::Preview => ResViewerTabs::Raw,
            Self::Raw => ResViewerTabs::Headers,
            Self::Headers => ResViewerTabs::Cookies,
            Self::Cookies => ResViewerTabs::Preview,
        }
    }

    pub fn prev(tab: &ResViewerTabs) -> Self {
        match tab {
            Self::Preview => ResViewerTabs::Cookies,
            Self::Raw => ResViewerTabs::Preview,
            Self::Headers => ResViewerTabs::Raw,
            Self::Cookies => ResViewerTabs::Headers,
        }
    }
}

impl From<ResViewerTabs> for usize {
    fn from(value: ResViewerTabs) -> Self {
        match value {
            ResViewerTabs::Preview => 0,
            ResViewerTabs::Raw => 1,
            ResViewerTabs::Headers => 2,
            ResViewerTabs::Cookies => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResViewerLayout {
    tabs_pane: Rect,
    content_pane: Rect,
    summary_pane: Rect,
}

#[derive(Debug, Clone)]
struct PreviewLayout {
    content_pane: Rect,
    scrollbar: Rect,
}

#[derive(Debug, Clone)]
pub struct ResponseViewer<'a> {
    colors: &'a hac_colors::Colors,
    response: Option<Rc<RefCell<Response>>>,
    tree: Option<Tree>,
    lines: Vec<Line<'static>>,
    error_lines: Option<Vec<Line<'static>>>,
    empty_lines: Vec<Line<'static>>,
    preview_layout: PreviewLayout,
    layout: ResViewerLayout,
    collection_store: Rc<RefCell<CollectionStore>>,
    active_tab: ResViewerTabs,
    raw_scroll: usize,
    raw_scroll_x: usize,
    raw_lines: Vec<String>,
    headers_scroll_y: usize,
    headers_scroll_x: usize,
    pretty_scroll: usize,
    pretty_scroll_x: usize,
    pretty_lines: Vec<String>,
    visual: bool,
    anchored: bool,
    cursor_initialized: bool,
    anchor: (usize, usize),
    cursor: (usize, usize),
    headers_selected: Option<usize>,
    flash: Option<(String, std::time::Instant)>,
}

impl<'a> ResponseViewer<'a> {
    pub fn new(
        colors: &'a hac_colors::Colors,
        collection_store: Rc<RefCell<CollectionStore>>,
        response: Option<Rc<RefCell<Response>>>,
        size: Rect,
    ) -> Self {
        let tree = response.as_ref().and_then(|response| {
            if let Some(ref pretty_body) = response.borrow().pretty_body {
                let pretty_body = pretty_body.to_string();
                let mut highlighter = HIGHLIGHTER.write().unwrap();
                highlighter.parse(&pretty_body)
            } else {
                None
            }
        });

        let layout = build_layout(size);
        let preview_layout = build_preview_layout(layout.content_pane);

        let empty_lines = make_empty_ascii_art(colors);

        ResponseViewer {
            colors,
            response,
            tree,
            lines: vec![],
            error_lines: None,
            empty_lines,
            preview_layout,
            layout,
            active_tab: ResViewerTabs::Preview,
            raw_scroll: 0,
            raw_scroll_x: 0,
            raw_lines: vec![],
            headers_scroll_y: 0,
            headers_scroll_x: 0,
            pretty_scroll: 0,
            pretty_scroll_x: 0,
            pretty_lines: vec![],
            visual: false,
            anchored: false,
            cursor_initialized: false,
            anchor: (0, 0),
            cursor: (0, 0),
            headers_selected: None,
            flash: None,
            collection_store,
        }
    }

    pub fn resize(&mut self, new_size: Rect) {
        self.layout = build_layout(new_size);
        self.preview_layout = build_preview_layout(self.layout.content_pane);
    }

    pub fn update(&mut self, response: Option<Rc<RefCell<Response>>>) {
        let body_str = response
            .as_ref()
            .and_then(|res| {
                res.borrow()
                    .pretty_body
                    .as_ref()
                    .map(|body| body.to_string())
            })
            .unwrap_or_default();

        if body_str.len().gt(&0) {
            self.tree = HIGHLIGHTER.write().unwrap().parse(&body_str);
            self.lines = build_syntax_highlighted_lines(&body_str, self.tree.as_ref(), self.colors);
        } else {
            self.tree = None;
            self.lines = vec![];
        }

        // cache the pretty body split on real line boundaries the same way we
        // cache the raw body; this is the substrate for selection on the
        // preview tab and must stay aligned with the styled lines
        self.pretty_lines = if body_str.is_empty() {
            vec![]
        } else {
            body_str
                .split('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
                .collect()
        };

        // a new response invalidates any pending selection and the cursor
        // memory, positions of a previous body are meaningless now
        self.visual = false;
        self.cursor_initialized = false;
        self.headers_selected = None;
        self.pretty_scroll_x = 0;

        // cache the raw body split on real line boundaries, CRLF line endings
        // are stripped of the trailing carriage return so lines are clean;
        // this is the source of truth for the raw tab and for selection
        self.raw_lines = response
            .as_ref()
            .and_then(|res| res.borrow().body.clone())
            .map(|body| {
                body.split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
                    .collect()
            })
            .unwrap_or_default();
        self.raw_scroll_x = 0;

        if let Some(res) = response.as_ref() {
            let cause: String = res
                .borrow()
                .cause
                .as_ref()
                .map(|cause| cause.to_string())
                .unwrap_or(String::default());

            self.error_lines = Some(
                get_error_ascii_art(
                    self.preview_layout.content_pane.width,
                    &mut rand::thread_rng(),
                )
                .iter()
                .map(|line| Line::from(line.to_string()).centered())
                .chain(vec!["".into()])
                .chain(
                    cause
                        .chars()
                        .collect::<Vec<_>>()
                        .chunks(self.layout.content_pane.width.sub(3).into())
                        .map(|chunk| {
                            Line::from(chunk.iter().collect::<String>().fg(self.colors.normal.red))
                        })
                        .collect::<Vec<_>>(),
                )
                .collect::<Vec<Line>>(),
            )
        };

        self.empty_lines = make_empty_ascii_art(self.colors);
        self.response = response;
    }

    fn draw_container(&self, size: Rect, frame: &mut Frame) {
        let is_focused = self
            .collection_store
            .borrow()
            .get_focused_pane()
            .eq(&PaneFocus::Preview);
        let is_selected = self
            .collection_store
            .borrow()
            .get_selected_pane()
            .is_some_and(|pane| pane.eq(&PaneFocus::Preview));

        let block_border = match (is_focused, is_selected) {
            (true, false) => Style::default().fg(self.colors.bright.blue),
            (true, true) => Style::default().fg(self.colors.normal.red),
            (_, _) => Style::default().fg(self.colors.bright.black),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(vec![
                "P".fg(self.colors.normal.red).bold(),
                "review".fg(self.colors.bright.black),
            ])
            .border_style(block_border);

        frame.render_widget(block, size);
    }

    fn draw_tabs(&self, frame: &mut Frame, size: Rect) {
        let tabs = Tabs::new(["Pretty", "Raw", "Headers", "Cookies"])
            .style(Style::default().fg(self.colors.bright.black))
            .select(self.active_tab.clone().into())
            .highlight_style(
                Style::default()
                    .fg(self.colors.normal.white)
                    .bg(self.colors.normal.blue),
            );
        frame.render_widget(tabs, size);
    }

    fn draw_spinner(&self, frame: &mut Frame) {
        let request_pane = self.preview_layout.content_pane;
        let center = request_pane.y.add(request_pane.height.div_ceil(2));
        let size = Rect::new(request_pane.x, center, request_pane.width, 1);
        let spinner = Spinner::default()
            .with_label("Sending request".fg(self.colors.bright.black))
            .with_style(Style::default().fg(self.colors.normal.red))
            .into_centered_line();

        frame.render_widget(Clear, request_pane);
        frame.render_widget(
            Block::default().bg(self.colors.primary.background),
            request_pane,
        );
        frame.render_widget(spinner, size);
    }

    fn draw_network_error(&self, frame: &mut Frame) {
        if self.response.as_ref().is_some() {
            let request_pane = self.preview_layout.content_pane;

            frame.render_widget(Clear, request_pane);
            frame.render_widget(
                Block::default().bg(self.colors.primary.background),
                request_pane,
            );

            let center = request_pane
                .y
                .add(request_pane.height.div_ceil(2))
                .sub(self.error_lines.as_ref().unwrap().len().div_ceil(2) as u16);

            let size = Rect::new(
                request_pane.x.add(1),
                center,
                request_pane.width,
                self.error_lines.as_ref().unwrap().len() as u16,
            );

            frame.render_widget(
                Paragraph::new(self.error_lines.clone().unwrap()).fg(self.colors.bright.black),
                size,
            )
        }
    }

    fn draw_waiting_for_request(&self, frame: &mut Frame) {
        let request_pane = self.preview_layout.content_pane;
        frame.render_widget(Clear, request_pane);
        frame.render_widget(
            Block::default().bg(self.colors.primary.background),
            request_pane,
        );

        let mut empty_message = self.empty_lines.clone();

        if self.empty_lines.len() >= request_pane.height.into() {
            empty_message = vec![
                "your handy API client".fg(self.colors.normal.red).into(),
                "".into(),
                "make a request and the result will appear here"
                    .fg(self.colors.normal.red)
                    .into(),
            ];
        }

        let center = request_pane
            .y
            .add(request_pane.height.div_ceil(2))
            .sub(empty_message.len().div_ceil(2) as u16);

        let size = Rect::new(
            request_pane.x.add(1),
            center,
            request_pane.width,
            self.empty_lines.len() as u16,
        );

        frame.render_widget(
            Paragraph::new(empty_message)
                .fg(self.colors.normal.red)
                .centered(),
            size,
        )
    }

    fn draw_current_tab(&mut self, frame: &mut Frame, size: Rect) -> anyhow::Result<()> {
        if self
            .response
            .as_ref()
            .is_some_and(|res| res.borrow().is_error)
        {
            self.draw_network_error(frame);
        };

        if self.response.is_none() {
            self.draw_waiting_for_request(frame);
        }

        if self
            .response
            .as_ref()
            .is_some_and(|res| !res.borrow().is_error)
        {
            match self.active_tab {
                ResViewerTabs::Preview => self.draw_pretty_response(frame, size),
                ResViewerTabs::Raw => self.draw_raw_response(frame, size),
                ResViewerTabs::Headers => self.draw_response_headers(frame),
                ResViewerTabs::Cookies => UnderConstruction::new(self.colors).draw(frame, size)?,
            }
        }

        if self.collection_store.borrow().has_pending_request() {
            self.draw_spinner(frame);
        }

        Ok(())
    }

    fn draw_response_headers(&mut self, frame: &mut Frame) {
        if let Some(response) = self.response.as_ref() {
            if let Some(headers) = response.borrow().headers.as_ref() {
                let mut longest_line: usize = 0;

                let mut lines: Vec<Line> = vec![
                    Line::from("Headers".fg(self.colors.normal.red).bold()),
                    Line::from(""),
                ];

                for (rendered_idx, (name, value)) in headers
                    .iter()
                    .filter(|(_, value)| value.to_str().is_ok())
                    .enumerate()
                {
                    let name_string = name.to_string();
                    let value = value.to_str().expect("filtered to valid UTF-8 values");
                    let aux = name_string.len().max(value.len());
                    longest_line = aux.max(longest_line);
                    lines.push(Line::from(
                        name_string
                            .chars()
                            .skip(self.headers_scroll_x)
                            .collect::<String>()
                            .bold()
                            .yellow(),
                    ));
                    let value_text = value
                        .chars()
                        .skip(self.headers_scroll_x)
                        .collect::<String>();
                    // the selected header has its value highlighted
                    let value_line =
                        if self.headers_selected.is_some_and(|idx| idx.eq(&rendered_idx)) {
                            Line::from(value_text.reversed())
                        } else {
                            Line::from(value_text)
                        };
                    lines.push(value_line);
                    lines.push(Line::from(""));
                }

                if self
                    .headers_scroll_y
                    // we add a blank line after every entry, we account for that here
                    .ge(&lines.len().saturating_sub(2))
                {
                    self.headers_scroll_y = lines.len().saturating_sub(2);
                }

                if self.headers_scroll_x.ge(&longest_line.saturating_sub(1)) {
                    self.headers_scroll_x = longest_line.saturating_sub(1);
                }

                let [headers_pane, x_scrollbar_pane] =
                    build_horizontal_scrollbar(self.preview_layout.content_pane);
                self.draw_scrollbar(
                    lines.len(),
                    self.headers_scroll_y,
                    frame,
                    self.preview_layout.scrollbar,
                );

                let lines_to_show =
                    if longest_line > self.preview_layout.content_pane.width as usize {
                        headers_pane.height
                    } else {
                        self.preview_layout.content_pane.height
                    };

                let lines = lines
                    .into_iter()
                    .skip(self.headers_scroll_y)
                    .chain(iter::repeat(Line::from("~".fg(self.colors.bright.black))))
                    .take(lines_to_show as usize)
                    .collect::<Vec<Line>>();

                let block = Block::default().padding(Padding::left(1));
                if longest_line > self.preview_layout.content_pane.width as usize {
                    self.draw_horizontal_scrollbar(
                        longest_line,
                        self.headers_scroll_x,
                        x_scrollbar_pane.width as usize,
                        frame,
                        x_scrollbar_pane,
                    );
                    frame.render_widget(Paragraph::new(lines).block(block), headers_pane)
                } else {
                    frame.render_widget(
                        Paragraph::new(lines).block(block),
                        self.preview_layout.content_pane,
                    );
                }
            }
        }
    }

    fn draw_raw_response(&mut self, frame: &mut Frame, _size: Rect) {
        if self.response.is_some() {
            let longest_line = self
                .raw_lines
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0);

            // clamp the horizontal scroll so `$` lands with the end of the
            // longest line at the right edge of the panel
            let max_scroll_x = longest_line.saturating_sub(self.viewport_width());
            if self.raw_scroll_x.gt(&max_scroll_x) {
                self.raw_scroll_x = max_scroll_x;
            }

            let lines = if !self.raw_lines.is_empty() {
                let width = self.viewport_width();
                self.raw_lines
                    .iter()
                    .enumerate()
                    .map(|(idx, line)| {
                        let reverse = self.selection_rev_range(idx);
                        let styled = Line::from(line.clone());
                        slice_styled_line(&styled, self.raw_scroll_x, width, reverse)
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![Line::from("No body").centered()]
            };
            // allow for scrolling down until theres only one line left into view
            if self.raw_scroll.ge(&lines.len().saturating_sub(1)) {
                self.raw_scroll = lines.len().saturating_sub(1);
            }

            let [raw_pane, x_scrollbar_pane] =
                build_horizontal_scrollbar(self.preview_layout.content_pane);

            // when the horizontal scrollbar is visible we lose one line of
            // content, so we account for it
            let lines_to_show =
                if longest_line > self.preview_layout.content_pane.width as usize {
                    raw_pane.height
                } else {
                    self.preview_layout.content_pane.height
                };

            self.draw_scrollbar(
                lines.len(),
                self.raw_scroll,
                frame,
                self.preview_layout.scrollbar,
            );

            if longest_line > self.preview_layout.content_pane.width as usize {
                self.draw_horizontal_scrollbar(
                    longest_line,
                    scrollbar_position(self.raw_scroll_x, longest_line, self.viewport_width()),
                    self.viewport_width(),
                    frame,
                    x_scrollbar_pane,
                );
            }

            let lines_in_view = lines
                .into_iter()
                .skip(self.raw_scroll)
                .chain(iter::repeat(Line::from("~".fg(self.colors.bright.black))))
                .take(lines_to_show as usize)
                .collect::<Vec<_>>();

            let raw_response = Paragraph::new(lines_in_view);
            frame.render_widget(raw_response, self.preview_layout.content_pane);
        }
    }

    fn draw_scrollbar(
        &self,
        total_lines: usize,
        current_scroll: usize,
        frame: &mut Frame,
        size: Rect,
    ) {
        let mut scrollbar_state = ScrollbarState::new(total_lines).position(current_scroll);

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(self.colors.normal.red))
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        frame.render_stateful_widget(scrollbar, size, &mut scrollbar_state);
    }

    fn draw_horizontal_scrollbar(
        &self,
        total_columns: usize,
        current_scroll: usize,
        viewport_length: usize,
        frame: &mut Frame,
        size: Rect,
    ) {
        let mut scrollbar_state = ScrollbarState::new(total_columns)
            .viewport_content_length(viewport_length)
            .position(current_scroll);

        let scrollbar = Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
            .style(Style::default().fg(self.colors.normal.red))
            .begin_symbol(Some("←"))
            .end_symbol(Some("→"));

        frame.render_stateful_widget(scrollbar, size, &mut scrollbar_state);
    }

    fn draw_pretty_response(&mut self, frame: &mut Frame, _size: Rect) {
        if self.response.as_ref().is_some() {
            let longest_line = self
                .pretty_lines
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0);

            // clamp the horizontal scroll so `$` lands with the end of the
            // longest line at the right edge of the panel
            let max_scroll_x = longest_line.saturating_sub(self.viewport_width());
            if self.pretty_scroll_x.gt(&max_scroll_x) {
                self.pretty_scroll_x = max_scroll_x;
            }

            if self.pretty_scroll.ge(&self.lines.len().saturating_sub(1)) {
                self.pretty_scroll = self.lines.len().saturating_sub(1);
            }

            let [pretty_pane, x_scrollbar_pane] =
                build_horizontal_scrollbar(self.preview_layout.content_pane);

            // when the horizontal scrollbar is visible we lose one line of
            // content, so we account for it
            let lines_to_show =
                if longest_line > self.preview_layout.content_pane.width as usize {
                    pretty_pane.height
                } else {
                    self.preview_layout.content_pane.height
                };

            self.draw_scrollbar(
                self.lines.len(),
                self.pretty_scroll,
                frame,
                self.preview_layout.scrollbar,
            );

            if longest_line > self.preview_layout.content_pane.width as usize {
                self.draw_horizontal_scrollbar(
                    longest_line,
                    scrollbar_position(
                        self.pretty_scroll_x,
                        longest_line,
                        self.viewport_width(),
                    ),
                    self.viewport_width(),
                    frame,
                    x_scrollbar_pane,
                );
            }

            let width = self.viewport_width();
            let lines = if self.lines.len().gt(&0) {
                self.lines
                    .iter()
                    .enumerate()
                    .map(|(idx, line)| {
                        // highlight the char under the cursor when the visual
                        // selection is active, when sitting past the last char
                        // of a line we highlight the last char instead
                        let reverse = self.selection_rev_range(idx);
                        slice_styled_line(line, self.pretty_scroll_x, width, reverse)
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![Line::from("No body").centered()]
            };

            let lines_in_view = lines
                .into_iter()
                .skip(self.pretty_scroll)
                .chain(iter::repeat(Line::from("~".fg(self.colors.bright.black))))
                .take(lines_to_show as usize)
                .collect::<Vec<_>>();

            let pretty_response = Paragraph::new(lines_in_view);
            frame.render_widget(pretty_response, self.preview_layout.content_pane);
        }
    }

    /// when the visual selection is active, returns the char range of the
    /// given line that falls within the selection, cursor char inclusive;
    /// when the selection is zero-width it degenerates to the cursor cell
    fn selection_rev_range(&self, line_idx: usize) -> Option<(usize, usize)> {
        if !self.visual {
            return None;
        }
        let start = self.anchor.min(self.cursor);
        let end = self.anchor.max(self.cursor);

        if line_idx.lt(&start.0) || line_idx.gt(&end.0) {
            return None;
        }

        Some(if start.0.eq(&end.0) {
            (start.1, end.1.add(1))
        } else if line_idx.eq(&start.0) {
            (start.1, usize::MAX)
        } else if line_idx.eq(&end.0) {
            (0, end.1.add(1))
        } else {
            (0, usize::MAX)
        })
    }

    /// toggles the visual selection; right after entering, the cursor moves
    /// around without a selection being drawn, until the anchor is dropped
    /// with o, so the start can be positioned first; the cursor remembers
    /// where the last selection left it, so consecutive selections pick up
    /// from the same place
    fn toggle_visual(&mut self) {
        self.visual = !self.visual;
        if self.visual {
            if !self.cursor_initialized {
                let scroll_y = match self.active_tab {
                    ResViewerTabs::Preview => self.pretty_scroll,
                    _ => self.raw_scroll,
                };
                self.cursor = (scroll_y, 0);
                self.cursor_initialized = true;
            }
            // the content may have changed since the last selection, so the
            // cursor is clamped against the current content
            self.clamp_cursor();
            self.anchor = self.cursor;
            self.anchored = false;
        }
    }

    /// the values of the headers rendered on the headers tab, in wire order;
    /// non UTF-8 values are skipped exactly like the rendering does, so the
    /// selection index can never desync from the screen
    fn rendered_headers(&self) -> Vec<String> {
        self.response
            .as_ref()
            .and_then(|res| res.borrow().headers.as_ref().map(|headers| {
                headers
                    .iter()
                    .filter_map(|(_, value)| value.to_str().ok().map(str::to_string))
                    .collect()
            }))
            .unwrap_or_default()
    }

    /// toggles the header-selection mode of the headers tab, selection moves
    /// whole headers at a time and the value is what gets copied; entering
    /// picks the first header visible under the current scroll
    fn toggle_header_selection(&mut self) {
        self.headers_selected = match self.headers_selected {
            Some(_) => None,
            None => {
                let count = self.rendered_headers().len();
                if count.eq(&0) {
                    None
                } else {
                    let first_visible = self.headers_scroll_y.saturating_sub(2) / 3;
                    Some(first_visible.min(count.sub(1)))
                }
            }
        };
    }

    /// moves the header selection up or down one whole header
    fn move_header_selection(&mut self, code: KeyCode) {
        let count = self.rendered_headers().len();
        let Some(idx) = self.headers_selected else {
            return;
        };
        self.headers_selected = Some(match code {
            KeyCode::Char('j') => idx.add(1).min(count.saturating_sub(1)),
            KeyCode::Char('k') => idx.saturating_sub(1),
            _ => idx,
        });
        self.follow_header_selection();
    }

    /// scrolls the headers tab so the selected header's name and value stay
    /// visible, every header takes three lines
    fn follow_header_selection(&mut self) {
        let Some(idx) = self.headers_selected else {
            return;
        };
        let name_line = idx * 3 + 2;
        let value_line = name_line + 1;
        // one line may be taken by the horizontal scrollbar
        let visible = (self.preview_layout.content_pane.height as usize).saturating_sub(1);
        if name_line.lt(&self.headers_scroll_y) {
            self.headers_scroll_y = name_line;
        } else if value_line.ge(&self.headers_scroll_y.add(visible)) {
            self.headers_scroll_y = name_line;
        }
    }

    /// clamps the cursor to the content of the active tab; the cursor always
    /// sits on a real char, never on the virtual end of a line, so every
    /// motion is visible
    fn clamp_cursor(&mut self) {
        let lines = match self.active_tab {
            ResViewerTabs::Preview => &self.pretty_lines,
            _ => &self.raw_lines,
        };
        let last_line = lines.len().saturating_sub(1);
        let line = self.cursor.0.min(last_line);
        let line_len = lines
            .get(line)
            .map(|line| line.chars().count())
            .unwrap_or(0);
        self.cursor = (line, self.cursor.1.min(line_len.saturating_sub(1)));
    }

    /// moves the cursor around the content of the active tab, then makes the
    /// viewport follow it
    fn visual_move(&mut self, code: KeyCode) {
        let (line, col) = self.cursor;
        self.cursor = match code {
            KeyCode::Char('j') => (line.add(1), col),
            KeyCode::Char('k') => (line.saturating_sub(1), col),
            KeyCode::Char('h') => (line, col.saturating_sub(1)),
            KeyCode::Char('l') => (line, col.add(1)),
            KeyCode::Char('0') => (line, 0),
            KeyCode::Char('$') => (line, self.active_line_len(line).saturating_sub(1)),
            _ => self.cursor,
        };
        self.clamp_cursor();
        // while the anchor hasn't been dropped yet the cursor moves around
        // freely without drawing a selection, the anchor follows it
        if !self.anchored {
            self.anchor = self.cursor;
        }
        self.follow_cursor();
    }

    fn active_line_len(&self, line: usize) -> usize {
        let lines = match self.active_tab {
            ResViewerTabs::Preview => &self.pretty_lines,
            _ => &self.raw_lines,
        };
        lines
            .get(line)
            .map(|line| line.chars().count())
            .unwrap_or(0)
    }

    /// scrolls the viewport of the active tab so the cursor stays visible,
    /// scrolling only when the cursor would move past an edge so it feels
    /// symmetric on both sides
    fn follow_cursor(&mut self) {
        let height = self.preview_layout.content_pane.height as usize;
        let width = self.viewport_width();
        let (line, col) = self.cursor;

        let (mut scroll_y, mut scroll_x) = match self.active_tab {
            ResViewerTabs::Preview => (self.pretty_scroll, self.pretty_scroll_x),
            ResViewerTabs::Raw => (self.raw_scroll, self.raw_scroll_x),
            _ => return,
        };

        if line.lt(&scroll_y) {
            scroll_y = line;
        }
        if line.ge(&scroll_y.add(height)) {
            scroll_y = line.saturating_sub(height.saturating_sub(1));
        }
        if col.lt(&scroll_x) {
            scroll_x = col;
        }
        if col.ge(&scroll_x.add(width)) {
            scroll_x = col.saturating_sub(width.saturating_sub(1));
        }

        match self.active_tab {
            ResViewerTabs::Preview => {
                self.pretty_scroll = scroll_y;
                self.pretty_scroll_x = scroll_x;
            }
            ResViewerTabs::Raw => {
                self.raw_scroll = scroll_y;
                self.raw_scroll_x = scroll_x;
            }
            _ => {}
        }
    }

    /// the width of the viewport in chars, the panes render exactly this many
    /// chars per line, borders and the vertical scrollbar excluded
    fn viewport_width(&self) -> usize {
        self.preview_layout.content_pane.width as usize
    }

    /// copies the response content to the system clipboard, flashing feedback
    /// on the summary line; the source matches the active tab: the pretty
    /// body on the preview tab, the headers as `Key: Value` lines on the
    /// headers tab, the raw body on the raw tab; the cookies tab is still
    /// under construction and has nothing to copy
    fn copy_response_body(&mut self) {
        let body = self
            .response
            .as_ref()
            .and_then(|res| {
                let res = res.borrow();
                match self.active_tab {
                    ResViewerTabs::Preview => res
                        .pretty_body
                        .as_ref()
                        .map(|pretty| pretty.to_string())
                        .or_else(|| res.body.clone()),
                    ResViewerTabs::Headers => res.headers.as_ref().map(|headers| {
                        headers
                            .iter()
                            .map(|(name, value)| {
                                format!("{name}: {}", String::from_utf8_lossy(value.as_bytes()))
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    }),
                    ResViewerTabs::Raw => res.body.clone(),
                    // the cookies tab is still under construction upstream, there is
                    // no cookie data to copy
                    ResViewerTabs::Cookies => None,
                }
            });

        match body {
            Some(body) => self.copy_to_clipboard(body, "to clipboard"),
            None => self.nothing_to_copy(),
        }
    }

    /// extracts the text covered by the active selection: the first line from
    /// the anchor column to its end, whole middle lines, and the last line up
    /// to the cursor column, cursor char inclusive
    fn selected_text(&self) -> Option<String> {
        if !self.visual {
            return None;
        }
        let lines = match self.active_tab {
            ResViewerTabs::Preview => &self.pretty_lines,
            _ => &self.raw_lines,
        };
        let (start, end) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));

        let mut out: Vec<String> = vec![];
        for idx in start.0..=end.0 {
            let line = lines.get(idx)?;
            let chars: Vec<char> = line.chars().collect();
            let from = if idx.eq(&start.0) { start.1 } else { 0 };
            let to = if idx.eq(&end.0) {
                end.1.add(1).min(chars.len())
            } else {
                chars.len()
            };
            let from = from.min(chars.len());
            out.push(chars[from..to.max(from)].iter().collect());
        }
        Some(out.join("\n"))
    }

    fn copy_to_clipboard(&mut self, text: String, label: &str) {
        let flash = if text.is_empty() {
            "Nothing to copy".to_string()
        } else {
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text))
            {
                Ok(()) => format!("Copied {label}"),
                Err(e) => {
                    tracing::error!("failed to copy to clipboard: {e}");
                    format!("Failed to copy: {e}")
                }
            }
        };
        self.flash = Some((flash, std::time::Instant::now()));
    }

    fn nothing_to_copy(&mut self) {
        self.flash = Some(("Nothing to copy".to_string(), std::time::Instant::now()));
    }

    fn draw_summary(&mut self, frame: &mut Frame, size: Rect) {
        if let Some(ref response) = self.response {
            let status_color = match response
                .borrow()
                .status
                .map(|status| status.as_u16())
                .unwrap_or_default()
            {
                s if s < 400 => self.colors.normal.green,
                _ => self.colors.normal.red,
            };

            let status = match response.borrow().status {
                Some(status) if size.width.gt(&50) => format!(
                    "{} ({})",
                    status.as_str(),
                    status
                        .canonical_reason()
                        .expect("tried to get the canonical_reason from an invalid status code")
                )
                .fg(status_color),
                Some(status) => status.as_str().to_string().fg(status_color),
                None => "Error".fg(self.colors.normal.red),
            };

            let mut pieces: Vec<Span> = vec![
                "Status: ".fg(self.colors.bright.black),
                status,
                " ".into(),
                "Time: ".fg(self.colors.bright.black),
                format!("{}ms", response.borrow().duration.as_millis())
                    .fg(self.colors.normal.green),
                " ".into(),
            ];

            if let Some(size) = response.borrow().size {
                pieces.push("Size: ".fg(self.colors.bright.black));
                pieces.push(format!("{} B", size).fg(self.colors.normal.green))
            };

            if let Some((message, at)) = self.flash.as_ref() {
                if at.elapsed().lt(&std::time::Duration::from_secs(2)) {
                    let color = if message.starts_with("Copied") {
                        self.colors.normal.green
                    } else {
                        self.colors.normal.red
                    };
                    pieces.push(" ".into());
                    pieces.push(" ".fg(self.colors.bright.black));
                    pieces.push(message.clone().fg(color));
                } else {
                    self.flash = None;
                }
            }

            frame.render_widget(Line::from(pieces), size);
        }
    }
}

impl<'a> Renderable for ResponseViewer<'a> {
    fn draw(&mut self, frame: &mut Frame, size: Rect) -> anyhow::Result<()> {
        self.draw_tabs(frame, self.layout.tabs_pane);
        self.draw_current_tab(frame, self.layout.content_pane)?;
        self.draw_summary(frame, self.layout.summary_pane);
        self.draw_container(size, frame);

        Ok(())
    }

    fn resize(&mut self, _new_size: Rect) {}
}

impl<'a> Eventful for ResponseViewer<'a> {
    type Result = ResponseViewerEvent;

    fn handle_key_event(&mut self, key_event: KeyEvent) -> anyhow::Result<Option<Self::Result>> {
        if let (KeyCode::Char('c'), KeyModifiers::CONTROL) = (key_event.code, key_event.modifiers) {
            return Ok(Some(ResponseViewerEvent::Quit));
        }

        // arrow keys are aliases of the hjkl motions everywhere on this pane
        let code = match key_event.code {
            KeyCode::Down => KeyCode::Char('j'),
            KeyCode::Up => KeyCode::Char('k'),
            KeyCode::Left => KeyCode::Char('h'),
            KeyCode::Right => KeyCode::Char('l'),
            code => code,
        };

        if let KeyCode::Esc = code {
            // while a selection is active esc cancels it instead of leaving
            // the pane
            if self.visual || self.headers_selected.is_some() {
                self.visual = false;
                self.headers_selected = None;
                return Ok(None);
            }
            return Ok(Some(ResponseViewerEvent::RemoveSelection));
        }

        if let KeyCode::Char('v') = code {
            match self.active_tab {
                ResViewerTabs::Preview | ResViewerTabs::Raw => self.toggle_visual(),
                // on the headers tab the selection moves whole headers and
                // the value is what gets highlighted and copied
                ResViewerTabs::Headers => self.toggle_header_selection(),
                ResViewerTabs::Cookies => {}
            }
        }

        // the header-selection only responds to up/down, horizontal panning
        // keeps its scrolling role while a value is highlighted
        if self.headers_selected.is_some()
            && matches!(self.active_tab, ResViewerTabs::Headers)
            && matches!(code, KeyCode::Char('j') | KeyCode::Char('k'))
        {
            self.move_header_selection(code);
            return Ok(None);
        }

        // while the visual selection is active o drops the anchor at the
        // cursor; until the first o the selection stays collapsed on the
        // cursor, and further o's reposition the start
        if self.visual
            && matches!(code, KeyCode::Char('o'))
            && matches!(self.active_tab, ResViewerTabs::Preview | ResViewerTabs::Raw)
        {
            self.anchor = self.cursor;
            self.anchored = true;
            return Ok(None);
        }

        // while the visual selection is active the motion keys move the
        // cursor instead of scrolling, the viewport follows the cursor
        if self.visual
            && matches!(self.active_tab, ResViewerTabs::Preview | ResViewerTabs::Raw)
            && matches!(
                code,
                KeyCode::Char('j')
                    | KeyCode::Char('k')
                    | KeyCode::Char('h')
                    | KeyCode::Char('l')
                    | KeyCode::Char('0')
                    | KeyCode::Char('$')
            )
        {
            self.visual_move(code);
            return Ok(None);
        }

        if let KeyCode::Tab = code {
            self.visual = false;
            self.headers_selected = None;
            self.active_tab = ResViewerTabs::next(&self.active_tab);
        }

        if let KeyCode::BackTab = key_event.code {
            self.visual = false;
            self.headers_selected = None;
            self.active_tab = ResViewerTabs::prev(&self.active_tab);
        }

        match code {
            KeyCode::Char('0') => match self.active_tab {
                ResViewerTabs::Headers => self.headers_scroll_x = 0,
                ResViewerTabs::Raw => self.raw_scroll_x = 0,
                ResViewerTabs::Preview => self.pretty_scroll_x = 0,
                ResViewerTabs::Cookies => {}
            },
            KeyCode::Char('$') => match self.active_tab {
                ResViewerTabs::Headers => self.headers_scroll_x = usize::MAX,
                ResViewerTabs::Raw => self.raw_scroll_x = usize::MAX,
                ResViewerTabs::Preview => self.pretty_scroll_x = usize::MAX,
                ResViewerTabs::Cookies => {}
            },
            KeyCode::Char('h') => match self.active_tab {
                ResViewerTabs::Headers => {
                    self.headers_scroll_x = self.headers_scroll_x.saturating_sub(1)
                }
                ResViewerTabs::Raw => self.raw_scroll_x = self.raw_scroll_x.saturating_sub(1),
                ResViewerTabs::Preview => {
                    self.pretty_scroll_x = self.pretty_scroll_x.saturating_sub(1)
                }
                ResViewerTabs::Cookies => {}
            },
            KeyCode::Char('l') => match self.active_tab {
                ResViewerTabs::Headers => self.headers_scroll_x = self.headers_scroll_x.add(1),
                ResViewerTabs::Raw => self.raw_scroll_x = self.raw_scroll_x.add(1),
                ResViewerTabs::Preview => self.pretty_scroll_x = self.pretty_scroll_x.add(1),
                ResViewerTabs::Cookies => {}
            },
            KeyCode::Char('j') => match self.active_tab {
                ResViewerTabs::Preview => self.pretty_scroll = self.pretty_scroll.add(1),
                ResViewerTabs::Raw => self.raw_scroll = self.raw_scroll.add(1),
                ResViewerTabs::Headers => self.headers_scroll_y = self.headers_scroll_y.add(1),
                ResViewerTabs::Cookies => {}
            },
            KeyCode::Char('k') => match self.active_tab {
                ResViewerTabs::Preview => self.pretty_scroll = self.pretty_scroll.saturating_sub(1),
                ResViewerTabs::Raw => self.raw_scroll = self.raw_scroll.saturating_sub(1),
                ResViewerTabs::Headers => {
                    self.headers_scroll_y = self.headers_scroll_y.saturating_sub(1)
                }
                ResViewerTabs::Cookies => {}
            },
            KeyCode::Char('y') => {
                // while a selection is active y copies it and exits the
                // selection, otherwise the whole content
                if let Some(idx) = self.headers_selected {
                    self.headers_selected = None;
                    let value = self
                        .rendered_headers()
                        .get(idx)
                        .cloned()
                        .unwrap_or_default();
                    self.copy_to_clipboard(value, "selection");
                } else if let Some(text) = self.selected_text() {
                    self.visual = false;
                    self.copy_to_clipboard(text, "selection");
                } else {
                    self.copy_response_body();
                }
            }
            _ => {}
        }

        Ok(None)
    }
}

fn build_layout(size: Rect) -> ResViewerLayout {
    let size = Rect::new(
        size.x.add(1),
        size.y.add(1),
        size.width.saturating_sub(2),
        size.height.saturating_sub(2),
    );

    let [tabs_pane, _, content_pane, summary_pane] = Layout::default()
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .direction(Direction::Vertical)
        .areas(size);

    ResViewerLayout {
        tabs_pane,
        content_pane,
        summary_pane,
    }
}

fn build_horizontal_scrollbar(size: Rect) -> [Rect; 2] {
    let [request_pane, _, scrollbar_pane] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(size);

    [request_pane, scrollbar_pane]
}

fn build_preview_layout(size: Rect) -> PreviewLayout {
    let [content_pane, _, scrollbar] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(size);

    PreviewLayout {
        content_pane,
        scrollbar,
    }
}

/// ratatui expects the thumb position of a scrollbar to travel all the way
/// to `content_length - 1` to reach the end of the track, while our scroll
/// range goes from 0 to `content - viewport`, so we remap the scroll
/// proportionally to the full range the scrollbar expects
fn scrollbar_position(scroll: usize, content: usize, viewport: usize) -> usize {
    let max_scroll = content.saturating_sub(viewport);
    if max_scroll.eq(&0) {
        return 0;
    }
    scroll.min(max_scroll) * content.saturating_sub(1) / max_scroll
}

/// slices a styled line down to the `[x, x + width)` viewport, reversing the
/// style of the `[reverse.0, reverse.1)` column range when given; all
/// coordinates are content coordinates in char units, so the caller must
/// clamp them to the line length beforehand
fn slice_styled_line(
    line: &Line<'static>,
    x: usize,
    width: usize,
    reverse: Option<(usize, usize)>,
) -> Line<'static> {
    let (rev_start, rev_end) = reverse.unwrap_or((usize::MAX, usize::MAX));
    let mut spans: Vec<Span> = vec![];
    let mut col = 0;

    for span in line.spans.iter() {
        let len = span.content.chars().count();
        if len.eq(&0) {
            continue;
        }
        let (start, end) = (col, col.add(len));
        col = end;

        // overlap of this span with the viewport
        let view_start = start.max(x);
        let view_end = end.min(x.add(width));
        if view_start.ge(&view_end) {
            continue;
        }

        // the portions of the viewport overlap that fall inside/outside the
        // reversed range, emitted in order
        let segments = [
            (view_start, view_end.min(rev_start), false),
            (view_start.max(rev_start), view_end.min(rev_end), true),
            (view_start.max(rev_end), view_end, false),
        ];

        for (seg_start, seg_end, reversed) in segments {
            if seg_start.ge(&seg_end) {
                continue;
            }
            let text: String = span
                .content
                .chars()
                .skip(seg_start.sub(start))
                .take(seg_end.sub(seg_start))
                .collect();
            let style = if reversed {
                span.style.add_modifier(Modifier::REVERSED)
            } else {
                span.style
            };
            spans.push(Span::styled(text, style));
        }
    }

    Line::from(spans)
}

fn get_error_ascii_art<R>(width: u16, rng: &mut R) -> &'static [&'static str]
where
    R: Rng,
{
    match width.gt(&60) {
        false => {
            let index = rng.gen_range(0..SMALL_ERROR_ARTS.len());
            SMALL_ERROR_ARTS[index]
        }
        true => {
            let full_range_arts = BIG_ERROR_ARTS
                .iter()
                .chain(SMALL_ERROR_ARTS)
                .collect::<Vec<_>>();
            let index = rng.gen_range(0..full_range_arts.len());
            full_range_arts[index]
        }
    }
}

fn make_empty_ascii_art(colors: &hac_colors::Colors) -> Vec<Line<'static>> {
    LOGO_ASCII[0]
        .iter()
        .map(|line| line.to_string().into())
        .chain(vec![
            "".into(),
            "your handy API client".fg(colors.bright.blue).into(),
            "".into(),
            "".into(),
            "make a request and the result will appear here"
                .fg(colors.bright.black)
                .into(),
        ])
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;
    #[test]
    fn test_ascii_with_size() {
        let seed = [0u8; 32];
        let mut rng = StdRng::from_seed(seed);

        let too_small = 59;
        let art = get_error_ascii_art(too_small, &mut rng);

        let expected = [
            r#"  ____  ____ ____ ___   ____ "#,
            r#" / _  )/ ___) ___) _ \ / ___)"#,
            r#"( (/ /| |  | |  | |_| | |    "#,
            r#" \____)_|  |_|   \___/|_|    "#,
        ];

        assert_eq!(art, expected);

        let expected = [
            r#"     dBBBP dBBBBBb  dBBBBBb    dBBBBP dBBBBBb"#,
            r#"               dBP      dBP   dBP.BP      dBP"#,
            r#"   dBBP    dBBBBK   dBBBBK   dBP.BP   dBBBBK "#,
            r#"  dBP     dBP  BB  dBP  BB  dBP.BP   dBP  BB "#,
            r#" dBBBBP  dBP  dB' dBP  dB' dBBBBP   dBP  dB' "#,
        ];

        let big_enough = 100;
        let art = get_error_ascii_art(big_enough, &mut rng);

        assert_eq!(art, expected);
    }
}
