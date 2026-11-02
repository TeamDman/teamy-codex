//! Selection and detail navigation over a bounded telemetry history.
use teamy_tracing::telemetry::StoreStats;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Screen {
    #[default]
    Overview,
    Detail {
        scroll: usize,
    },
}

/// Selection uses the store's accepted-record sequence, not a shifting index.
#[derive(Debug, Default)]
pub struct Navigation {
    pub selected: Option<u64>,
    pub screen: Screen,
}

impl Navigation {
    pub fn initialize_selection(&mut self, stats: StoreStats) {
        if self.selected.is_none() && stats.records > 0 {
            self.selected = Some(stats.accepted);
        }
    }

    #[must_use]
    pub fn selected_index(&self, stats: StoreStats) -> Option<usize> {
        if stats.records == 0 {
            return None;
        }
        let first = stats
            .accepted
            .saturating_sub(stats.records as u64)
            .saturating_add(1);
        let selected = self.selected?;
        if selected < first || selected > stats.accepted {
            return None;
        }
        usize::try_from(selected - first).ok()
    }

    pub fn previous(&mut self, stats: StoreStats) {
        match &mut self.screen {
            Screen::Detail { scroll } => *scroll = scroll.saturating_sub(1),
            Screen::Overview if stats.records > 0 => {
                let first = stats
                    .accepted
                    .saturating_sub(stats.records as u64)
                    .saturating_add(1);
                let selected = self.selected.unwrap_or(stats.accepted);
                self.selected = Some(selected.saturating_sub(1).clamp(first, stats.accepted));
            }
            Screen::Overview => {}
        }
    }

    pub fn next(&mut self, stats: StoreStats) {
        match &mut self.screen {
            Screen::Detail { scroll } => *scroll = scroll.saturating_add(1),
            Screen::Overview if stats.records > 0 => {
                let first = stats
                    .accepted
                    .saturating_sub(stats.records as u64)
                    .saturating_add(1);
                let selected = self.selected.unwrap_or(first);
                self.selected = Some(selected.saturating_add(1).clamp(first, stats.accepted));
            }
            Screen::Overview => {}
        }
    }

    pub fn enter_detail(&mut self, stats: StoreStats) {
        if self.selected_index(stats).is_some() {
            self.screen = Screen::Detail { scroll: 0 };
        }
    }

    /// Returns true when already at the root and the caller should exit.
    pub fn back(&mut self) -> bool {
        if self.screen == Screen::Overview {
            true
        } else {
            self.screen = Screen::Overview;
            false
        }
    }
}
