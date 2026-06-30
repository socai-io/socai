use std::io::{self, IsTerminal};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use socai_core::agent::tool::{ToolProgressEvent, ToolProgressPhase, ToolProgressStatus};

pub struct ProgressRenderer {
    ui: Option<ProgressUi>,
}

impl ProgressRenderer {
    pub fn new(show_reading: bool, show_ocr: bool, total: u64) -> Self {
        let enabled = io::stderr().is_terminal() && (show_reading || show_ocr);
        Self {
            ui: enabled.then(|| ProgressUi::new(show_reading, show_ocr, total)),
        }
    }

    pub fn update(&mut self, event: ToolProgressEvent) {
        if let Some(ui) = &mut self.ui {
            ui.update(event);
        }
    }

    pub fn finish(&mut self) {
        if let Some(ui) = &mut self.ui {
            ui.finish();
        }
    }
}

fn truncate_title(title: &str) -> String {
    const MAX_CHARS: usize = 64;
    let sanitized: String = title
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let sanitized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = sanitized.chars();
    let prefix: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

struct ProgressUi {
    multi: MultiProgress,
    status: ProgressBar,
    reading: Option<ProgressBar>,
    ocr: Option<ProgressBar>,
    reading_finished: bool,
}

impl ProgressUi {
    fn new(show_reading: bool, show_ocr: bool, total: u64) -> Self {
        let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stderr());
        // Build the full layout before the first draw. Dynamically inserting
        // the OCR bar after Reading has already rendered leaves stale lines in
        // some terminals.
        let reading = show_reading.then(|| multi.add(progress_bar("Reading", total)));
        let ocr = show_ocr.then(|| multi.add(progress_bar("Local OCR", total)));
        let status = multi.add(ProgressBar::new_spinner());
        status.set_style(
            ProgressStyle::with_template("{msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        Self {
            multi,
            status,
            reading,
            ocr,
            reading_finished: !show_reading,
        }
    }

    fn update(&mut self, event: ToolProgressEvent) {
        let bar = match event.phase {
            ToolProgressPhase::Reading => self.reading.as_ref(),
            ToolProgressPhase::Ocr => self.ocr.as_ref(),
        };
        let Some(bar) = bar else {
            return;
        };
        bar.set_length(event.total);
        bar.set_position(event.current.min(event.total));

        match (event.phase, event.status) {
            (ToolProgressPhase::Reading, ToolProgressStatus::ItemStarted) => {
                if let Some(title) = event.title.as_deref() {
                    self.status
                        .set_message(format!("Reading: {}", truncate_title(title)));
                }
            }
            (ToolProgressPhase::Reading, ToolProgressStatus::Finished) => {
                self.reading_finished = true;
                self.status.set_message("Finalizing results…");
                bar.finish();
            }
            (ToolProgressPhase::Ocr, ToolProgressStatus::ItemStarted) if self.reading_finished => {
                if let Some(title) = event.title.as_deref() {
                    self.status
                        .set_message(format!("Running OCR: {}", truncate_title(title)));
                }
            }
            (ToolProgressPhase::Ocr, ToolProgressStatus::Finished) => {
                bar.finish();
            }
            _ => {}
        }
    }

    fn finish(&mut self) {
        if let Some(bar) = &self.reading {
            bar.finish_and_clear();
        }
        if let Some(bar) = &self.ocr {
            bar.finish_and_clear();
        }
        self.status.finish_and_clear();
        let _ = self.multi.clear();
    }
}

fn progress_bar(label: &str, total: u64) -> ProgressBar {
    let bar = ProgressBar::new(total);
    bar.set_prefix(label.to_string());
    bar.set_style(
        ProgressStyle::with_template("{prefix:>10} [{bar:32}] {pos:>3}/{len:3}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
    );
    bar
}
