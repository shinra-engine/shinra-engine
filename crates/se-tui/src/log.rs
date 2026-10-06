//! The Terminal panel's lines.
//!
//! An entry is exactly one terminal line, because ratatui does not break a
//! span on `'\n'` — a rustc error pushed whole would draw as one garbled row.
//! So a block is split on the way in, and a long one is cut short: the first
//! lines of a compiler error are the ones that say what went wrong.

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub text: String,
    pub level: Level,
}

/// Entries kept in total; the oldest go first.
const CAP: usize = 200;
/// Entries one pushed block may take.
const BLOCK: usize = 12;

#[derive(Default)]
pub struct Log {
    lines: Vec<LogLine>,
}

impl Log {
    pub fn push(&mut self, s: &str, level: Level) {
        let s = s.strip_suffix('\n').unwrap_or(s);
        let all: Vec<&str> = s.split('\n').collect();
        let line = |text: String| LogLine { text, level };
        if all.len() > BLOCK {
            let keep = BLOCK - 1;
            self.lines.extend(all[..keep].iter().map(|t| line(t.to_string())));
            self.lines.push(line(format!("... (+{} more lines)", all.len() - keep)));
        } else {
            self.lines.extend(all.iter().map(|t| line(t.to_string())));
        }
        if self.lines.len() > CAP {
            self.lines.drain(..self.lines.len() - CAP);
        }
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &LogLine> + ExactSizeIterator {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_one_entry_per_line() {
        let mut log = Log::default();
        log.push("a\nb", Level::Info);
        let got: Vec<_> = log.iter().cloned().collect();
        assert_eq!(
            got,
            vec![
                LogLine { text: "a".into(), level: Level::Info },
                LogLine { text: "b".into(), level: Level::Info },
            ]
        );
    }

    #[test]
    fn a_long_block_is_cut_to_twelve() {
        let mut log = Log::default();
        let block: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        log.push(&block.join("\n"), Level::Error);
        assert_eq!(log.len(), 12);
        let last = log.iter().last().unwrap();
        assert_eq!(last.text, "... (+19 more lines)");
        assert_eq!(last.level, Level::Error);
        assert_eq!(log.iter().nth(10).unwrap().text, "line 10");
    }

    #[test]
    fn the_newest_two_hundred_are_kept() {
        let mut log = Log::default();
        for i in 0..250 {
            let level = if i % 2 == 0 { Level::Info } else { Level::Error };
            log.push(&format!("{i}"), level);
        }
        assert_eq!(log.len(), 200);
        let first = log.iter().next().unwrap();
        assert_eq!(first.text, "50");
        assert_eq!(first.level, Level::Info);
        let last = log.iter().next_back().unwrap();
        assert_eq!(last.text, "249");
        assert_eq!(last.level, Level::Error);
    }
}
