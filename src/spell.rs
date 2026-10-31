//! Spell checking for writing. The dictionary is compiled in, so it works with
//! nothing installed, and a person's own words live in `words.txt` beside the
//! settings file.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::ops::Range;
use std::path::PathBuf;

/// The words of `line` worth checking, as byte ranges. Left out: web and mail
/// addresses, LaTeX commands, anything with a digit, and names written in
/// capitals or mixed case, such as `LSTM` and `arXiv`, which no dictionary has.
pub fn words(line: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for token in line.split_whitespace() {
        if token.contains("://") || token.contains('@') || token.starts_with("www.") {
            continue;
        }
        let base = token.as_ptr() as usize - line.as_ptr() as usize;
        add_words(token, base, &mut out);
    }
    out
}

fn add_words(token: &str, base: usize, out: &mut Vec<Range<usize>>) {
    let chars: Vec<(usize, char)> = token.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_alphanumeric() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && part_of_word(&chars, i) {
            i += 1;
        }
        let from = chars[start].0;
        let to = chars.get(i).map_or(token.len(), |c| c.0);
        let after_backslash = start > 0 && chars[start - 1].1 == '\\';
        if !after_backslash && worth_checking(&token[from..to]) {
            out.push(base + from..base + to);
        }
    }
}

/// A letter or digit, or an apostrophe with a letter on both sides.
fn part_of_word(chars: &[(usize, char)], i: usize) -> bool {
    let c = chars[i].1;
    let letter_at = |n: usize| chars.get(n).is_some_and(|c| c.1.is_alphabetic());
    c.is_alphanumeric()
        || (matches!(c, '\'' | '’') && i > 0 && letter_at(i - 1) && letter_at(i + 1))
}

fn worth_checking(word: &str) -> bool {
    let mut letters = word.chars();
    let first = letters.next();
    word.chars().count() > 1
        && word.chars().all(|c| !c.is_numeric())
        && first.is_some_and(char::is_alphabetic)
        && !letters.any(char::is_uppercase)
}

/// A curly apostrophe reads the same but isn't in the dictionary.
fn plain(word: &str) -> String {
    word.replace('’', "'")
}

/// The checker, with the words added this session and those to leave alone.
#[cfg(feature = "spell")]
pub struct Speller {
    dictionary: spellbook::Dictionary,
    ignored: HashSet<String>,
    words_file: PathBuf,
}

#[cfg(feature = "spell")]
impl Speller {
    /// Loads the dictionary and any words in `words_file`.
    pub fn new(words_file: PathBuf) -> Option<Speller> {
        let aff = include_str!("../vendor/dictionaries/en_US/en_US.aff");
        let dic = include_str!("../vendor/dictionaries/en_US/en_US.dic");
        let mut dictionary = spellbook::Dictionary::new(aff, dic).ok()?;
        for word in fs::read_to_string(&words_file).unwrap_or_default().lines() {
            let _ = dictionary.add(word.trim());
        }
        Some(Speller {
            dictionary,
            ignored: HashSet::new(),
            words_file,
        })
    }

    pub fn is_wrong(&self, word: &str) -> bool {
        let word = plain(word);
        !self.ignored.contains(&word) && !self.dictionary.check(&word)
    }

    /// Likely spellings, best first.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut found = Vec::new();
        self.dictionary.suggest(&plain(word), &mut found);
        found
    }

    /// Leaves `word` alone until Kyte closes.
    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(plain(word));
    }

    /// Adds `word` for good. It works for the session even if the file can't
    /// be written.
    pub fn add(&mut self, word: &str) -> io::Result<()> {
        let word = plain(word);
        self.ignored.insert(word.clone());
        if let Some(dir) = self.words_file.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.words_file)?;
        writeln!(file, "{word}")
    }
}

/// Stands in for the checker in builds without it.
#[cfg(not(feature = "spell"))]
pub struct Speller {
    ignored: HashSet<String>,
    words_file: PathBuf,
}

#[cfg(not(feature = "spell"))]
impl Speller {
    pub fn new(_words_file: PathBuf) -> Option<Speller> {
        None
    }

    pub fn is_wrong(&self, _word: &str) -> bool {
        false
    }

    pub fn suggest(&self, _word: &str) -> Vec<String> {
        Vec::new()
    }

    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(plain(word));
    }

    pub fn add(&mut self, word: &str) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.words_file)?;
        writeln!(file, "{word}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(line: &str) -> Vec<&str> {
        words(line).into_iter().map(|r| &line[r]).collect()
    }

    #[test]
    fn words_keep_their_apostrophes_and_lose_their_punctuation() {
        assert_eq!(
            found("Don't stop, (please) it's dogs' \"fine\"."),
            ["Don't", "stop", "please", "it's", "dogs", "fine"]
        );
        assert_eq!(found("the author’s word"), ["the", "author’s", "word"]);
    }

    #[test]
    fn hyphens_split_words_so_each_part_is_checked() {
        assert_eq!(found("well-known"), ["well", "known"]);
    }

    #[test]
    fn addresses_commands_and_names_are_left_alone() {
        assert_eq!(found("see https://example.com/teh now"), ["see", "now"]);
        assert_eq!(found("mail a@example.com or www.teh.org"), ["mail", "or"]);
        assert_eq!(found("\\section*{Intro} \\emph{bold}"), ["Intro", "bold"]);
        assert_eq!(found("LSTM arXiv GPUs x2 3rd a I"), Vec::<&str>::new());
    }

    #[test]
    fn ranges_are_bytes_of_the_line_and_survive_wide_text() {
        let line = "naïve  café";
        let r = words(line);
        assert_eq!(&line[r[0].clone()], "naïve");
        assert_eq!(&line[r[1].clone()], "café");
    }

    #[cfg(feature = "spell")]
    fn speller(dir: &tempfile::TempDir) -> Speller {
        Speller::new(dir.path().join("cfg/words.txt")).unwrap()
    }

    #[cfg(feature = "spell")]
    #[test]
    fn the_dictionary_knows_right_from_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let s = speller(&dir);
        for word in [
            "hello",
            "walking",
            "don't",
            "don’t",
            "eigenvalue",
            "Fourier",
        ] {
            assert!(!s.is_wrong(word), "{word}");
        }
        for word in ["teh", "recieve", "experimant"] {
            assert!(s.is_wrong(word), "{word}");
        }
        assert_eq!(
            s.suggest("recieve").first().map(String::as_str),
            Some("receive")
        );
    }

    #[cfg(feature = "spell")]
    #[test]
    fn ignoring_lasts_the_session_and_adding_lasts() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = speller(&dir);
        s.ignore("teh");
        assert!(!s.is_wrong("teh"));
        s.add("Kyteword").unwrap();
        assert!(!s.is_wrong("Kyteword"));
        let again = speller(&dir);
        assert!(again.is_wrong("teh"), "ignoring doesn't last");
        assert!(!again.is_wrong("Kyteword"), "adding does");
    }

    #[cfg(feature = "spell")]
    #[test]
    fn a_word_is_added_even_if_the_file_cant_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("file");
        fs::write(&blocked, "").unwrap();
        let mut s = Speller::new(blocked.join("words.txt")).unwrap();
        assert!(s.add("Kyteword").is_err());
        assert!(!s.is_wrong("Kyteword"));
    }
}
