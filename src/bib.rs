//! Reading BibTeX files for citations. It's a small scanner rather than the
//! grammar, so a half-written entry still leaves the others usable, and so it
//! works in builds without highlighting.

use std::path::{Path, PathBuf};

/// One entry of a bibliography.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub key: String,
    pub author: String,
    pub title: String,
    pub year: String,
}

impl Reference {
    /// One line for a list: the key, then what to recognize the work by.
    pub fn line(&self) -> String {
        let mut about = String::new();
        if !self.author.is_empty() {
            about.push_str(&self.author);
        }
        if !self.title.is_empty() {
            if !about.is_empty() {
                about.push_str(", ");
            }
            about.push_str(&self.title);
        }
        if !self.year.is_empty() {
            about.push_str(&format!(" ({})", self.year));
        }
        if about.is_empty() {
            self.key.clone()
        } else {
            format!("{}  {}", self.key, about.trim_start())
        }
    }
}

/// The byte after the `}` or `)` matching the one at `open`.
fn past_closing(bytes: &[u8], open: usize) -> Option<usize> {
    let close = if bytes[open] == b'(' { b')' } else { b'}' };
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if b == bytes[open] {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// Reads a field's value at `at`: its text, and where it ends.
fn value(text: &str, at: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    match *bytes.get(at)? {
        b'{' => {
            let end = past_closing(bytes, at)?;
            Some((text[at + 1..end - 1].to_string(), end))
        }
        b'"' => {
            let mut depth = 0usize;
            let mut i = at + 1;
            loop {
                match *bytes.get(i)? {
                    b'{' => depth += 1,
                    b'}' => depth = depth.saturating_sub(1),
                    b'"' if depth == 0 => break,
                    _ => {}
                }
                i += 1;
            }
            Some((text[at + 1..i].to_string(), i + 1))
        }
        _ => {
            let n = bytes[at..]
                .iter()
                .position(|b| matches!(b, b',' | b'}' | b')') || b.is_ascii_whitespace())?;
            Some((text[at..at + n].to_string(), at + n))
        }
    }
}

fn clean(value: &str) -> String {
    let no_braces: String = value.chars().filter(|c| !matches!(c, '{' | '}')).collect();
    no_braces.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A short form of an author list: `Knuth`, or `Knuth et al.`.
fn short_author(authors: &str) -> String {
    let authors = clean(authors);
    let mut people = authors.split(" and ");
    let first = people.next().unwrap_or_default();
    let family = match first.split_once(',') {
        Some((family, _)) => family,
        None => first.rsplit(' ').next().unwrap_or_default(),
    };
    if people.next().is_some() {
        format!("{} et al.", family.trim())
    } else {
        family.trim().to_string()
    }
}

/// Reads the entry whose `{` or `(` is at `open`, up to `end`.
fn entry(text: &str, open: usize, end: usize) -> Option<Reference> {
    let bytes = text.as_bytes();
    let after = open + 1;
    let comma = after + text[after..end].find(',').unwrap_or(end - 1 - after);
    let key = text[after..comma].trim().to_string();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let mut found = Reference {
        key,
        author: String::new(),
        title: String::new(),
        year: String::new(),
    };
    let mut at = comma + 1;
    while at < end - 1 {
        at = skip_space(bytes, at);
        let Some(equals) = text[at..end].find('=').map(|i| at + i) else {
            break;
        };
        let name = text[at..equals].trim().to_ascii_lowercase();
        let (mut text_value, mut next) = value(text, skip_space(bytes, equals + 1))?;
        // `"a" # "b"` joins strings.
        loop {
            let hash = skip_space(bytes, next);
            if bytes.get(hash) != Some(&b'#') {
                break;
            }
            let (more, after_more) = value(text, skip_space(bytes, hash + 1))?;
            text_value.push_str(&more);
            next = after_more;
        }
        match name.as_str() {
            "author" | "editor" if found.author.is_empty() => {
                found.author = short_author(&text_value);
            }
            "title" => found.title = clean(&text_value),
            "year" => found.year = clean(&text_value),
            "date" if found.year.is_empty() => {
                found.year = clean(&text_value).chars().take(4).collect();
            }
            _ => {}
        }
        at = skip_space(bytes, next);
        if bytes.get(at) == Some(&b',') {
            at += 1;
        }
    }
    Some(found)
}

/// The references in BibTeX `text`. An entry that can't be read is skipped.
pub fn parse(text: &str) -> Vec<Reference> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find('@') {
        let start = at + i;
        let name_end = start
            + 1
            + bytes[start + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphabetic())
                .count();
        let open = skip_space(bytes, name_end);
        at = start + 1;
        if name_end == start + 1 || !matches!(bytes.get(open), Some(b'{' | b'(')) {
            continue;
        }
        let Some(end) = past_closing(bytes, open) else {
            continue;
        };
        at = end;
        let kind = text[start + 1..name_end].to_ascii_lowercase();
        if matches!(kind.as_str(), "comment" | "string" | "preamble") {
            continue;
        }
        out.extend(entry(text, open, end));
    }
    out
}

/// The text of `\name{...}` wherever it appears, on lines not commented out.
fn command_args<'a>(tex: &'a str, name: &str) -> Vec<&'a str> {
    let pattern = format!("\\{name}");
    let mut out = Vec::new();
    for line in tex.lines().filter(|l| !l.trim_start().starts_with('%')) {
        for (i, _) in line.match_indices(&pattern) {
            let rest = &line[i + pattern.len()..];
            let rest = match rest.strip_prefix('[') {
                Some(r) => r.split_once(']').map_or("", |p| p.1),
                None => rest,
            };
            if let Some((arg, _)) = rest.strip_prefix('{').and_then(|r| r.split_once('}')) {
                out.push(arg);
            }
        }
    }
    out
}

/// The bibliographies a LaTeX file names, as paths under `dir`. It leaves the
/// `.bib` off `\bibliography`, and uses one name per `\addbibresource`.
pub fn named_files(tex: &str, dir: &Path) -> Vec<PathBuf> {
    let classic = command_args(tex, "bibliography");
    let classic = classic.iter().flat_map(|arg| arg.split(','));
    let modern = command_args(tex, "addbibresource");
    classic
        .chain(modern)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let mut path = dir.join(name);
            if path.extension().is_none() {
                path.set_extension("bib");
            }
            path
        })
        .collect()
}

/// Whether `before`, the text ahead of the cursor, ends inside the braces of a
/// citation command: `\cite{`, `\citep[p. 3]{`, `\autocite{a,`.
pub fn in_citation(before: &str) -> bool {
    let Some(open) = before.rfind('{') else {
        return false;
    };
    if before[open..].contains('}') {
        return false;
    }
    let mut head = before[..open].trim_end();
    // Optional notes in brackets: `\cite[see][p. 3]{`.
    while let Some(rest) = head.strip_suffix(']') {
        let Some(i) = rest.rfind('[') else {
            return false;
        };
        head = rest[..i].trim_end();
    }
    let head = head.strip_suffix('*').unwrap_or(head);
    let name_start = head.rfind('\\').map_or(head.len(), |i| i + 1);
    let name = &head[name_start..];
    name_start > 0
        && name.chars().all(|c| c.is_ascii_alphabetic())
        && name.to_ascii_lowercase().contains("cite")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFS: &str = r#"
% a comment with an @ sign
@string{jacm = "Journal of the ACM"}
@comment{ignored}

@article{knuth84,
  author = {Knuth, Donald E.},
  title  = "{Literate} Programming",
  journal = jacm,
  year   = 1984,
}

@book(lamport94,
  title = {{\LaTeX}: A Document
           Preparation System},
  author = {Leslie Lamport and Duane Bibby},
  date = {1994-01-01},
)

@misc{broken, title = {never closed

@misc{bare}
"#;

    #[test]
    fn entries_are_read_in_every_shape() {
        let refs = parse(REFS);
        let keys: Vec<&str> = refs.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys[..2], ["knuth84", "lamport94"]);
        assert_eq!(refs[0].author, "Knuth");
        assert_eq!(refs[0].title, "Literate Programming");
        assert_eq!(refs[0].year, "1984");
        assert_eq!(refs[1].author, "Lamport et al.");
        assert_eq!(refs[1].title, "\\LaTeX: A Document Preparation System");
        assert_eq!(refs[1].year, "1994");
    }

    #[test]
    fn a_broken_entry_leaves_those_before_it_usable() {
        assert!(parse(REFS).iter().all(|r| r.key != "broken"));
        assert_eq!(parse("@article{a, title = {x").len(), 0);
        assert_eq!(parse("@").len(), 0);
        assert_eq!(parse("@article").len(), 0);
        assert_eq!(parse("@article{").len(), 0);
        assert_eq!(parse("mail me@example.com {please}").len(), 0);
    }

    #[test]
    fn strings_join_with_a_hash() {
        let refs = parse(r#"@misc{a, title = "one " # "two", year = 2020}"#);
        assert_eq!(refs[0].title, "one two");
        assert_eq!(refs[0].year, "2020");
    }

    #[test]
    fn an_entry_with_only_a_key_is_kept() {
        let refs = parse("@misc{bare}");
        assert_eq!(refs[0].key, "bare");
        assert_eq!(refs[0].line(), "bare");
    }

    #[test]
    fn non_ascii_text_is_safe() {
        let refs = parse("@book{é, author = {Gödel, Kurt}, title = {Über}}");
        assert_eq!(refs[0].author, "Gödel");
        assert_eq!(refs[0].line(), "é  Gödel, Über");
    }

    #[test]
    fn a_line_says_what_the_work_is() {
        let r = &parse(REFS)[0];
        assert_eq!(r.line(), "knuth84  Knuth, Literate Programming (1984)");
    }

    #[test]
    fn bibliographies_come_from_either_command() {
        let tex = "\\bibliography{refs, more/other}\n\\addbibresource[label=a]{x.bib}\n% \\bibliography{off}\n";
        let found = named_files(tex, Path::new("/p"));
        assert_eq!(
            found,
            [
                PathBuf::from("/p/refs.bib"),
                PathBuf::from("/p/more/other.bib"),
                PathBuf::from("/p/x.bib"),
            ]
        );
    }

    #[test]
    fn citation_commands_are_recognized() {
        for yes in [
            "\\cite{",
            "see \\cite{",
            "\\citep{",
            "\\citet*{",
            "\\autocite{",
            "\\parencite[see][p. 3]{",
            "\\Cite{",
            "\\cite{a,",
            "\\nocite{",
        ] {
            assert!(in_citation(yes), "{yes}");
        }
        for no in [
            "{",
            "cite{",
            "\\section{",
            "\\cite{a}",
            "\\cite{a} and {",
            "\\cite",
        ] {
            assert!(!in_citation(no), "{no}");
        }
    }
}
