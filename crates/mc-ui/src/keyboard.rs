//! Two-button text entry (`docs/spec/ui.md` §7): a ring of candidates, A moves through it, a
//! long A inserts the current one, B deletes.
//!
//! Order of the ring: quick replies (empty text only), word completions (your recent words
//! first, then the built-in French and English lists), space, letters by frequency (accents
//! right after their base letter), digits, punctuation, then the final action (`Send`, `Save`,
//! `Add`) last, so that a double click from the first candidate reaches it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

const FR: &str = include_str!("wordlist/fr.txt");
const EN: &str = include_str!("wordlist/en.txt");

/// Letters by frequency in French and English messages; accented ones after their base.
const LETTERS: &str = "eéèêaàâsitnrulodcçmpvqfbghjxyzwkœôûîïëü";
const DIGITS: &str = "0123456789";
const PUNCT: &str = ".,?!'-:@#/()+&";

/// Default quick replies (`docs/spec/ui.md` §6).
pub const QUICK_REPLIES: [&str; 8] = [
    "OK",
    "Yes",
    "No",
    "On my way",
    "Call me",
    "Where are you?",
    "Running late",
    "Thanks",
];

/// What the text is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A message; `limit` in bytes.
    Message { limit: usize },
    /// The node's name (31 bytes).
    Name,
    /// A `#hashtag` channel name (without the `#`).
    Hashtag,
    /// A server password: shown, lower and upper case, no completions.
    Password,
}

impl Mode {
    pub fn limit(self) -> usize {
        match self {
            Mode::Message { limit } => limit,
            Mode::Name => 31,
            Mode::Hashtag => 30,
            Mode::Password => 15,
        }
    }

    /// The final action's label.
    pub fn action(self) -> &'static str {
        match self {
            Mode::Message { .. } => "Send",
            Mode::Name => "Save",
            Mode::Hashtag => "Add",
            Mode::Password => "Login",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cand {
    /// The final action.
    Done,
    Space,
    Char(char),
    /// Completes the word being typed (and adds a space).
    Word(String),
    /// Replaces the whole text.
    Reply(&'static str),
}

impl Cand {
    pub fn label(&self, mode: Mode) -> String {
        match self {
            Cand::Done => String::from(mode.action()),
            Cand::Space => String::from("space"),
            Cand::Char(c) => c.to_string(),
            Cand::Word(w) => w.clone(),
            Cand::Reply(r) => String::from(*r),
        }
    }
}

/// What the screen does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Typing,
    /// The final action was chosen with this text.
    Done(String),
    /// B on an empty text.
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Keyboard {
    pub mode: Mode,
    pub text: String,
    pub sel: usize,
    cands: Vec<Cand>,
    recent: Vec<String>,
}

/// The word being typed: after the last space.
fn partial(text: &str) -> &str {
    text.rsplit(' ').next().unwrap_or("")
}

/// Upper-case the next letter: at the start and after a sentence.
fn sentence_start(text: &str) -> bool {
    let t = text.trim_end();
    t.is_empty() || (text.ends_with(' ') && t.ends_with(['.', '!', '?']))
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

impl Keyboard {
    /// `recent`: words from the messages sent from the device, most recent first.
    pub fn new(mode: Mode, text: &str, recent: Vec<String>) -> Self {
        let mut k = Self {
            mode,
            text: String::from(text),
            sel: 0,
            cands: Vec::new(),
            recent,
        };
        k.refresh();
        k
    }

    pub fn candidates(&self) -> &[Cand] {
        &self.cands
    }

    pub fn selected(&self) -> Option<&Cand> {
        self.cands.get(self.sel)
    }

    pub fn bytes(&self) -> usize {
        self.text.len()
    }

    fn completions(&self) -> Vec<String> {
        let p = partial(&self.text);
        if p.is_empty() || !matches!(self.mode, Mode::Message { .. }) {
            return Vec::new();
        }
        let lower = p.to_lowercase();
        let upper = p.chars().next().is_some_and(char::is_uppercase);
        let mut out: Vec<String> = Vec::new();
        let words = self
            .recent
            .iter()
            .map(String::as_str)
            .chain(FR.lines().zip(EN.lines()).flat_map(|(a, b)| [a, b]))
            .chain(FR.lines().skip(EN.lines().count()));
        for w in words {
            if out.len() >= 4 {
                break;
            }
            let wl = w.to_lowercase();
            if wl.len() > lower.len() && wl.starts_with(&lower) {
                let shown = if upper { capitalize(&wl) } else { wl };
                if !out.contains(&shown) {
                    out.push(shown);
                }
            }
        }
        out
    }

    fn refresh(&mut self) {
        let mut c: Vec<Cand> = Vec::new();
        let msg = matches!(self.mode, Mode::Message { .. });
        if msg && self.text.is_empty() {
            c.extend(QUICK_REPLIES.iter().map(|r| Cand::Reply(r)));
        }
        c.extend(self.completions().into_iter().map(Cand::Word));
        let can_space = !self.text.is_empty() && !self.text.ends_with(' ');
        if can_space && self.mode != Mode::Hashtag {
            c.push(Cand::Space);
        }
        let upper =
            matches!(self.mode, Mode::Message { .. } | Mode::Name) && sentence_start(&self.text);
        let ascii_only = matches!(self.mode, Mode::Hashtag | Mode::Password);
        for ch in LETTERS.chars() {
            if ascii_only && !ch.is_ascii_lowercase() {
                continue;
            }
            let ch = if upper {
                ch.to_uppercase().next().unwrap_or(ch)
            } else {
                ch
            };
            c.push(Cand::Char(ch));
        }
        if self.mode == Mode::Password {
            c.extend(
                LETTERS
                    .chars()
                    .filter(char::is_ascii_lowercase)
                    .map(|ch| Cand::Char(ch.to_ascii_uppercase())),
            );
        }
        c.extend(DIGITS.chars().map(Cand::Char));
        match self.mode {
            Mode::Hashtag => c.push(Cand::Char('-')),
            Mode::Name => c.extend("-_.'".chars().map(Cand::Char)),
            Mode::Message { .. } | Mode::Password => c.extend(PUNCT.chars().map(Cand::Char)),
        }
        if !self.text.trim().is_empty() {
            c.push(Cand::Done);
        }
        self.cands = c;
        self.sel = 0;
    }

    pub fn next(&mut self, n: usize) {
        if !self.cands.is_empty() {
            self.sel = (self.sel + n) % self.cands.len();
        }
    }

    pub fn prev(&mut self) {
        if !self.cands.is_empty() {
            self.sel = (self.sel + self.cands.len() - 1) % self.cands.len();
        }
    }

    fn fits(&self, extra: &str) -> bool {
        self.text.len() + extra.len() <= self.mode.limit()
    }

    /// Long A: insert the selected candidate (or finish).
    pub fn insert(&mut self) -> Key {
        let Some(c) = self.cands.get(self.sel).cloned() else {
            return Key::Typing;
        };
        match c {
            Cand::Done => return Key::Done(String::from(self.text.trim())),
            Cand::Space => {
                if self.fits(" ") {
                    self.text.push(' ');
                }
            }
            Cand::Char(ch) => {
                let mut b = [0u8; 4];
                if self.fits(ch.encode_utf8(&mut b)) {
                    self.text.push(ch);
                }
            }
            Cand::Word(w) => {
                let keep = self.text.len() - partial(&self.text).len();
                let mut t = String::from(self.text.get(..keep).unwrap_or(""));
                t.push_str(&w);
                t.push(' ');
                if t.len() <= self.mode.limit() {
                    self.text = t;
                }
            }
            Cand::Reply(r) => {
                if r.len() <= self.mode.limit() {
                    self.text = String::from(r);
                }
                self.refresh();
                // A quick reply is complete: the cursor goes to Send.
                self.sel = self
                    .cands
                    .iter()
                    .position(|c| *c == Cand::Done)
                    .unwrap_or(0);
                return Key::Typing;
            }
        }
        self.refresh();
        Key::Typing
    }

    /// B click: delete the last character, or cancel on an empty text.
    pub fn delete(&mut self) -> Key {
        if self.text.pop().is_none() {
            return Key::Cancel;
        }
        self.refresh();
        Key::Typing
    }
}

/// Distinct words of `texts`, most recent first (texts are given oldest first).
pub fn recent_words<'a>(texts: impl DoubleEndedIterator<Item = &'a [u8]>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in texts.rev() {
        let s = String::from_utf8_lossy(t);
        for w in s.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-') {
            let w = w.to_lowercase();
            if w.chars().count() >= 3 && !out.contains(&w) {
                out.push(w);
                if out.len() >= 64 {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pick(k: &mut Keyboard, want: &Cand) {
        let i = k.candidates().iter().position(|c| c == want).unwrap();
        k.sel = i;
        k.insert();
    }

    #[test]
    fn quick_reply_then_send_is_two_long_presses() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        assert_eq!(k.selected(), Some(&Cand::Reply("OK")));
        k.insert();
        assert_eq!(k.text, "OK");
        assert_eq!(k.selected(), Some(&Cand::Done));
        assert_eq!(k.insert(), Key::Done(String::from("OK")));
    }

    #[test]
    fn typing_capitalizes_and_completes_words() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        pick(&mut k, &Cand::Char('B'));
        pick(&mut k, &Cand::Char('o'));
        let w = k
            .candidates()
            .iter()
            .find_map(|c| match c {
                Cand::Word(w) => Some(w.clone()),
                _ => None,
            })
            .unwrap();
        assert!(w.starts_with("Bo"), "{w}");
        pick(&mut k, &Cand::Word(String::from("Bonjour")));
        assert_eq!(k.text, "Bonjour ");
        // Double click from the first candidate: the last one, Send.
        k.prev();
        assert_eq!(k.selected(), Some(&Cand::Done));
        assert_eq!(k.insert(), Key::Done(String::from("Bonjour")));
    }

    #[test]
    fn recent_words_come_first_and_limit_is_bytes() {
        let recent = recent_words([b"meet at the refuge".as_slice()].into_iter());
        let mut k = Keyboard::new(Mode::Message { limit: 3 }, "", recent);
        pick(&mut k, &Cand::Char('R'));
        assert_eq!(
            k.candidates().iter().find(|c| matches!(c, Cand::Word(_))),
            Some(&Cand::Word(String::from("Refuge")))
        );
        pick(&mut k, &Cand::Char('é'));
        assert_eq!(k.text, "Ré", "3 bytes");
        pick(&mut k, &Cand::Char('a'));
        assert_eq!(k.text, "Ré", "a fourth byte does not fit");
        assert_eq!(k.delete(), Key::Typing);
        assert_eq!(k.delete(), Key::Typing);
        assert_eq!(k.delete(), Key::Cancel);
    }

    #[test]
    fn hashtag_mode_is_lowercase_ascii() {
        let k = Keyboard::new(Mode::Hashtag, "", vec![]);
        assert!(k.candidates().iter().all(|c| match c {
            Cand::Char(ch) => ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '-',
            _ => false,
        }));
    }
}
