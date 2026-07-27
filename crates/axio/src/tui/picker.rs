//! Choosing a model from what the provider says it serves.
//!
//! The list is fetched, never compiled in. A hardcoded catalogue is wrong the
//! first time either side ships a model, and wrong in the direction nobody
//! checks: a name missing from a picker looks exactly like a name the provider
//! refuses.
//!
//! Pure state. Which entry is highlighted, and what a digit selects, are
//! functions of a list and an index, so they are tested without a terminal.

/// A list to choose from, and where the highlight sits in it.
#[derive(Debug)]
pub struct Picker {
    models: Vec<String>,
    selected: usize,
    /// The one in use, marked in the list. Kept rather than looked up, because
    /// the running model may not be in the listing at all — a name typed by
    /// hand, or one withdrawn since the session started — and the mark is then
    /// simply absent rather than wrong.
    current: String,
}

impl Picker {
    /// Open on the model in use, or at the top when it is not listed.
    ///
    /// Opening at the top regardless would put the highlight on a different
    /// model from the one running, and the first Enter — the reflex when a list
    /// appears where you expected a confirmation — would change it.
    pub fn new(models: Vec<String>, current: impl Into<String>) -> Self {
        let current = current.into();
        let selected = models.iter().position(|m| *m == current).unwrap_or(0);
        Self {
            models,
            selected,
            current,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn index(&self) -> usize {
        self.selected.min(self.models.len().saturating_sub(1))
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    pub fn selection(&self) -> Option<&str> {
        self.models.get(self.index()).map(String::as_str)
    }

    /// Move the highlight, wrapping at both ends.
    pub fn step(&mut self, delta: isize) {
        if self.models.is_empty() {
            return;
        }
        let at = self.index() as isize + delta;
        self.selected = at.rem_euclid(self.models.len() as isize) as usize;
    }

    /// Jump to a numbered entry, as the list is drawn.
    ///
    /// One-based, because the list is numbered from one on screen. Out of range
    /// is ignored rather than clamped: `9` on a list of three meaning "the
    /// third" is a surprise, and this is a keystroke that changes the model.
    pub fn choose_digit(&mut self, digit: u32) -> bool {
        let Some(at) = (digit as usize).checked_sub(1) else {
            return false;
        };
        if at >= self.models.len() {
            return false;
        }
        self.selected = at;
        true
    }

    /// The slice to draw, where it starts, and it follows the highlight.
    ///
    /// The viewport gives this about three rows and a provider can serve
    /// dozens, so the list scrolls rather than being cut off at the third.
    pub fn window(&self, rows: usize) -> (usize, &[String]) {
        if rows == 0 || self.models.is_empty() {
            return (0, &[]);
        }
        let last_start = self.models.len().saturating_sub(rows);
        let first = self
            .index()
            .saturating_sub(rows.saturating_sub(1))
            .min(last_start);
        let end = (first + rows).min(self.models.len());
        (first, &self.models[first..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<String> {
        ["alpha", "beta", "gamma", "delta"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    /// The reflex when a list appears is to press Enter. If that lands on
    /// anything but the model already running, the picker changed it by being
    /// opened.
    #[test]
    fn it_opens_on_the_model_in_use() {
        let picker = Picker::new(models(), "gamma");
        assert_eq!(picker.selection(), Some("gamma"));
    }

    #[test]
    fn a_model_missing_from_the_listing_opens_at_the_top() {
        // Typed by hand, or withdrawn since the session started.
        let picker = Picker::new(models(), "not-listed");
        assert_eq!(picker.selection(), Some("alpha"));
        assert_eq!(picker.current(), "not-listed");
    }

    #[test]
    fn the_highlight_wraps() {
        let mut picker = Picker::new(models(), "alpha");
        picker.step(-1);
        assert_eq!(picker.selection(), Some("delta"));
        picker.step(1);
        assert_eq!(picker.selection(), Some("alpha"));
    }

    #[test]
    fn digits_pick_the_entry_they_number() {
        let mut picker = Picker::new(models(), "alpha");
        assert!(picker.choose_digit(3));
        assert_eq!(picker.selection(), Some("gamma"));
    }

    /// `9` on a list of four must not mean "the last one". This keystroke
    /// changes the model, so a near-miss has to do nothing.
    #[test]
    fn a_digit_past_the_end_is_ignored_rather_than_clamped() {
        let mut picker = Picker::new(models(), "beta");
        assert!(!picker.choose_digit(9));
        assert!(!picker.choose_digit(0));
        assert_eq!(picker.selection(), Some("beta"), "nothing moved");
    }

    #[test]
    fn the_window_follows_the_highlight() {
        let picker = Picker::new(models(), "delta");
        let (first, shown) = picker.window(2);
        assert_eq!(shown.len(), 2);
        assert_eq!(first + shown.len(), 4, "the last entry must be on screen");
        assert_eq!(shown.last().map(String::as_str), Some("delta"));
    }

    #[test]
    fn an_empty_listing_is_not_a_panic() {
        let mut picker = Picker::new(Vec::new(), "alpha");
        assert!(picker.is_empty());
        assert_eq!(picker.selection(), None);
        picker.step(1);
        assert!(!picker.choose_digit(1));
        assert!(picker.window(3).1.is_empty());
    }
}
